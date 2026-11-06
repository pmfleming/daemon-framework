#[cfg(test)]
mod performance_tests;

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchItem {
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub score: f64,
    #[serde(default, rename = "providerPriority")]
    pub provider_priority: f64,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub owner: String,
    pub generation: u64,
    pub keys: Vec<String>,
}

#[derive(Debug)]
struct RankedItem<'a> {
    item: &'a SearchItem,
    match_score: i64,
}

/// Search-only projection prepared once when a catalog is replaced. Raw and
/// normalized field copies are not both retained by the worker.
#[derive(Debug)]
pub struct PreparedCatalog {
    items: Vec<SearchItem>,
}

impl PreparedCatalog {
    pub fn new(items: &[SearchItem]) -> Self {
        Self {
            items: items
                .iter()
                .map(|item| SearchItem {
                    key: item.key.clone(),
                    title: normalize(&item.title),
                    subtitle: normalize(&item.subtitle),
                    keywords: item.keywords.iter().map(|value| normalize(value)).collect(),
                    score: item.score,
                    provider_priority: item.provider_priority,
                })
                .collect(),
        }
    }

    pub fn rank(&self, owner: String, generation: u64, query: &str) -> SearchResponse {
        rank_prepared(owner, generation, query, &self.items)
    }
}

fn normalize(value: &str) -> String {
    value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

fn words(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn allowed_typos(length: usize) -> usize {
    match length {
        0..=3 => 0,
        4..=6 => 1,
        7..=10 => 2,
        _ => 3,
    }
}

fn token_score(field: &str, token: &str) -> Option<i64> {
    if field == token {
        return Some(14_000);
    }
    if field.starts_with(token) {
        return Some(12_500 - field.chars().count().min(500) as i64);
    }
    if let Some(index) = words(field).position(|word| word.starts_with(token)) {
        return Some(11_500 - index as i64 * 20);
    }
    if let Some(index) = field.find(token) {
        return Some(10_000 - index.min(500) as i64);
    }

    // Keep a fresh matcher: 0.3.7's cross-input matrix cache changes some
    // Unicode scores, and use_cache(false) panics on its RefCell borrow.
    let mut best = SkimMatcherV2::default()
        .fuzzy_match(field, token)
        .map(|score| 7_000 + score);
    let token_length = token.chars().count();
    let typo_limit = allowed_typos(token_length);
    // Exact/prefix/substring cases already returned above. A zero-edit match
    // cannot improve the remaining fuzzy result, and edit distance is bounded
    // below by the difference in Unicode scalar counts.
    if typo_limit == 0 {
        return best;
    }
    for word in words(field) {
        let length_difference = word.chars().count().abs_diff(token_length);
        if length_difference > typo_limit {
            continue;
        }
        let distance = strsim::osa_distance(word, token);
        if distance <= typo_limit {
            let score = 9_000 - distance as i64 * 900 - length_difference as i64 * 25;
            best = Some(best.map_or(score, |current| current.max(score)));
        }
    }
    best
}

fn best_token_score(title: &str, subtitle: &str, keywords: &[String], token: &str) -> Option<i64> {
    let keyword = keywords
        .iter()
        .filter_map(|field| token_score(field, token))
        .max();
    [
        token_score(title, token).map(|score| score + 1_800),
        token_score(subtitle, token).map(|score| score + 600),
        keyword,
    ]
    .into_iter()
    .flatten()
    .max()
}

fn title_bonus(title: &str, query: &str) -> i64 {
    if title == query {
        20_000
    } else if title.starts_with(query) {
        12_000
    } else if title.contains(query) {
        8_000
    } else {
        0
    }
}

fn item_match(item: &SearchItem, query: &str, tokens: &[&str]) -> Option<i64> {
    if tokens.is_empty() {
        return Some(0);
    }
    let score = tokens.iter().try_fold(0, |total, token| {
        best_token_score(&item.title, &item.subtitle, &item.keywords, token)
            .map(|score| total + score)
    })?;
    Some(score + title_bonus(&item.title, query))
}

pub fn rank(owner: String, generation: u64, query: &str, items: &[SearchItem]) -> SearchResponse {
    PreparedCatalog::new(items).rank(owner, generation, query)
}

fn rank_prepared(
    owner: String,
    generation: u64,
    query: &str,
    items: &[SearchItem],
) -> SearchResponse {
    let query = normalize(query);
    let query = query.trim();
    let tokens = words(query).collect::<Vec<_>>();
    let mut ranked = items
        .iter()
        .filter_map(|item| {
            let match_score = item_match(item, query, &tokens)?;
            Some(RankedItem { item, match_score })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .match_score
            .cmp(&left.match_score)
            .then_with(|| right.item.score.total_cmp(&left.item.score))
            .then_with(|| {
                right
                    .item
                    .provider_priority
                    .total_cmp(&left.item.provider_priority)
            })
            .then_with(|| left.item.title.cmp(&right.item.title))
            .then_with(|| left.item.key.cmp(&right.item.key))
    });
    SearchResponse {
        owner,
        generation,
        keys: ranked
            .into_iter()
            .map(|ranked| ranked.item.key.clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchItem, rank};

    fn item(key: &str, title: &str) -> SearchItem {
        SearchItem {
            key: key.into(),
            title: title.into(),
            subtitle: String::new(),
            keywords: Vec::new(),
            score: 0.0,
            provider_priority: 0.0,
        }
    }

    fn keys(query: &str, items: Vec<SearchItem>) -> Vec<String> {
        rank("test".into(), 1, query, &items).keys
    }

    #[test]
    fn empty_queries_use_score_priority_then_normalized_title_and_key() {
        let mut priority = item("priority", "Zulu");
        priority.provider_priority = 0.5;
        let mut scored = item("scored", "Zulu");
        scored.score = 0.25;
        assert_eq!(
            keys(
                "  ",
                vec![item("b", "Café"), item("a", "Cafe"), priority, scored]
            ),
            ["scored", "priority", "a", "b"]
        );
    }

    #[test]
    fn matches_normalized_substrings_and_rejects_unrelated_items() {
        assert_eq!(
            keys("fox", vec![item("firefox", "Mozilla Firefox")]),
            ["firefox"]
        );
        assert_eq!(keys("ffx", vec![item("firefox", "Firefox")]), ["firefox"]);
        assert_eq!(keys("cafe", vec![item("cafe", "Café")]), ["cafe"]);
        assert!(keys("terminal", vec![item("files", "Files")]).is_empty());
    }

    #[test]
    fn tolerates_omissions_substitutions_and_transpositions() {
        let values = vec![item("firefox", "Firefox"), item("files", "Files")];
        assert_eq!(keys("firfox", values), ["firefox"]);
        for (query, key, title) in [
            ("chorme", "chrome", "Google Chrome"),
            ("firwfox", "firefox", "Firefox"),
        ] {
            assert_eq!(keys(query, vec![item(key, title)]), [key]);
        }
    }

    #[test]
    fn all_tokens_must_match_and_strong_matches_rank_first() {
        let values = vec![
            item("code", "Visual Studio Code"),
            item("codium", "VSCodium"),
            item("contacts", "Google Contacts"),
        ];
        assert_eq!(keys("studio code", values), ["code"]);
        let ranked = keys(
            "code",
            vec![item("middle", "Barcode Tool"), item("exact", "Code")],
        );
        assert_eq!(ranked, ["exact", "middle"]);
    }
}
