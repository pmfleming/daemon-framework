use std::io::{self, BufRead, Write};

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Debug, Deserialize)]
struct SearchRequest {
    owner: String,
    generation: u64,
    #[serde(default)]
    query: String,
    #[serde(default)]
    items: Vec<SearchItem>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    key: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    score: i64,
    #[serde(default, rename = "providerPriority")]
    provider_priority: i64,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    owner: String,
    generation: u64,
    keys: Vec<String>,
}

#[derive(Debug)]
struct RankedItem {
    item: SearchItem,
    normalized_title: String,
    match_score: i64,
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

    let mut best = SkimMatcherV2::default()
        .fuzzy_match(field, token)
        .map(|score| 7_000 + score);
    let token_length = token.chars().count();
    let typo_limit = allowed_typos(token_length);
    for word in words(field) {
        let distance = strsim::osa_distance(word, token);
        if distance <= typo_limit {
            let score = 9_000
                - distance as i64 * 900
                - word.chars().count().abs_diff(token_length) as i64 * 25;
            best = Some(best.map_or(score, |current| current.max(score)));
        }
    }
    best
}

fn item_match(item: &SearchItem, query: &str) -> Option<(i64, String)> {
    let title = normalize(&item.title);
    if query.is_empty() {
        return Some((0, title));
    }

    let subtitle = normalize(&item.subtitle);
    let keywords = item
        .keywords
        .iter()
        .map(|value| normalize(value))
        .collect::<Vec<_>>();
    let tokens = words(query).collect::<Vec<_>>();
    if tokens.is_empty() {
        return Some((0, title));
    }

    let mut total = 0;
    for token in tokens {
        let title_score = token_score(&title, token).map(|score| score + 1_800);
        let subtitle_score = token_score(&subtitle, token).map(|score| score + 600);
        let keyword_score = keywords
            .iter()
            .filter_map(|field| token_score(field, token))
            .max();
        total += title_score
            .into_iter()
            .chain(subtitle_score)
            .chain(keyword_score)
            .max()?;
    }

    if title == query {
        total += 20_000;
    } else if title.starts_with(query) {
        total += 12_000;
    } else if title.contains(query) {
        total += 8_000;
    }
    Some((total, title))
}

fn rank(request: SearchRequest) -> SearchResponse {
    let query = normalize(&request.query);
    let query = query.trim();
    let mut ranked = request
        .items
        .into_iter()
        .filter_map(|item| {
            let (match_score, normalized_title) = item_match(&item, query)?;
            Some(RankedItem {
                item,
                normalized_title,
                match_score,
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .match_score
            .cmp(&left.match_score)
            .then_with(|| right.item.score.cmp(&left.item.score))
            .then_with(|| {
                right
                    .item
                    .provider_priority
                    .cmp(&left.item.provider_priority)
            })
            .then_with(|| left.normalized_title.cmp(&right.normalized_title))
            .then_with(|| left.item.key.cmp(&right.item.key))
    });
    SearchResponse {
        owner: request.owner,
        generation: request.generation,
        keys: ranked.into_iter().map(|ranked| ranked.item.key).collect(),
    }
}

pub fn serve() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SearchRequest>(&line) {
            Ok(request) => serde_json::to_writer(&mut stdout, &rank(request))?,
            Err(error) => serde_json::to_writer(
                &mut stdout,
                &serde_json::json!({ "error": format!("invalid search request: {error}") }),
            )?,
        }
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SearchItem, SearchRequest, rank};

    fn item(key: &str, title: &str) -> SearchItem {
        SearchItem {
            key: key.into(),
            title: title.into(),
            subtitle: String::new(),
            keywords: Vec::new(),
            score: 0,
            provider_priority: 0,
        }
    }

    fn keys(query: &str, items: Vec<SearchItem>) -> Vec<String> {
        rank(SearchRequest {
            owner: "test".into(),
            generation: 1,
            query: query.into(),
            items,
        })
        .keys
    }

    #[test]
    fn finds_middle_substrings_and_ordered_characters() {
        assert_eq!(
            keys("fox", vec![item("firefox", "Mozilla Firefox")]),
            ["firefox"]
        );
        assert_eq!(keys("ffx", vec![item("firefox", "Firefox")]), ["firefox"]);
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

    #[test]
    fn normalizes_diacritics_without_matching_unrelated_items() {
        assert_eq!(keys("cafe", vec![item("cafe", "Café")]), ["cafe"]);
        assert!(keys("terminal", vec![item("files", "Files")]).is_empty());
    }
}
