use std::io::{self, BufRead, Write};

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

fn char_count(value: &str) -> usize {
    value.chars().count()
}

/// Optimal ordered-character score, following the dynamic-programming approach
/// used by fzf's V2 matcher. Consecutive characters and word boundaries win;
/// long gaps are penalized. Unlike a substring search this finds `ffx` in
/// `Firefox` and acronyms such as `gc` in `Google Contacts`.
fn subsequence_score(candidate: &str, needle: &str) -> Option<i64> {
    let candidate = candidate.chars().collect::<Vec<_>>();
    let needle = needle.chars().collect::<Vec<_>>();
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() == 1 || needle.len() > candidate.len() {
        return None;
    }

    const UNMATCHED: i64 = i64::MIN / 4;
    let mut previous = vec![UNMATCHED; candidate.len()];
    for (candidate_index, character) in candidate.iter().enumerate() {
        if *character == needle[0] {
            let boundary =
                candidate_index == 0 || !candidate[candidate_index - 1].is_alphanumeric();
            previous[candidate_index] = 75 - candidate_index as i64 + if boundary { 35 } else { 0 };
        }
    }

    for needle_character in needle.iter().skip(1) {
        let mut current = vec![UNMATCHED; candidate.len()];
        let mut best_before = UNMATCHED;
        for candidate_index in 0..candidate.len() {
            if candidate_index > 0 {
                best_before = best_before.max(previous[candidate_index - 1]);
            }
            if candidate[candidate_index] != *needle_character || best_before == UNMATCHED {
                continue;
            }
            let boundary =
                candidate_index == 0 || !candidate[candidate_index - 1].is_alphanumeric();
            let general = best_before + 55 - candidate_index as i64;
            let consecutive = if candidate_index > 0 && previous[candidate_index - 1] != UNMATCHED {
                previous[candidate_index - 1] + 105
            } else {
                UNMATCHED
            };
            current[candidate_index] = general.max(consecutive) + if boundary { 30 } else { 0 };
        }
        previous = current;
    }

    previous
        .into_iter()
        .max()
        .filter(|score| *score != UNMATCHED)
}

/// Restricted Damerau-Levenshtein distance. Adjacent transpositions count as
/// one edit, which makes common typing errors such as `chorme` useful without
/// making short queries noisy.
fn edit_distance(left: &str, right: &str, maximum: usize) -> Option<usize> {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > maximum {
        return None;
    }

    let mut previous_previous = vec![0; right.len() + 1];
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for left_index in 1..=left.len() {
        let mut current = vec![left_index; right.len() + 1];
        let mut row_minimum = left_index;
        for right_index in 1..=right.len() {
            let substitution = previous[right_index - 1]
                + usize::from(left[left_index - 1] != right[right_index - 1]);
            current[right_index] = (previous[right_index] + 1)
                .min(current[right_index - 1] + 1)
                .min(substitution);
            if left_index > 1
                && right_index > 1
                && left[left_index - 1] == right[right_index - 2]
                && left[left_index - 2] == right[right_index - 1]
            {
                current[right_index] =
                    current[right_index].min(previous_previous[right_index - 2] + 1);
            }
            row_minimum = row_minimum.min(current[right_index]);
        }
        if row_minimum > maximum {
            return None;
        }
        previous_previous = previous;
        previous = current;
    }
    (previous[right.len()] <= maximum).then_some(previous[right.len()])
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
        return Some(12_500 - char_count(field).min(500) as i64);
    }
    if let Some(index) = words(field).position(|word| word.starts_with(token)) {
        return Some(11_500 - index as i64 * 20);
    }
    if let Some(index) = field.find(token) {
        return Some(10_000 - index.min(500) as i64);
    }

    let mut best = subsequence_score(field, token).map(|score| 7_000 + score);
    let typo_limit = allowed_typos(char_count(token));
    if typo_limit > 0 {
        for word in words(field) {
            if let Some(distance) = edit_distance(word, token, typo_limit) {
                let score = 9_000
                    - distance as i64 * 900
                    - char_count(word).abs_diff(char_count(token)) as i64 * 25;
                best = Some(best.map_or(score, |current| current.max(score)));
            }
        }
    }
    best
}

fn item_match_score(item: &SearchItem, query: &str) -> Option<i64> {
    let query = normalize(query).trim().to_owned();
    if query.is_empty() {
        return Some(0);
    }

    let title = normalize(&item.title);
    let subtitle = normalize(&item.subtitle);
    let keywords = item
        .keywords
        .iter()
        .map(|value| normalize(value))
        .collect::<Vec<_>>();
    let tokens = words(&query).collect::<Vec<_>>();
    if tokens.is_empty() {
        return Some(0);
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
    } else if title.starts_with(&query) {
        total += 12_000;
    } else if title.contains(&query) {
        total += 8_000;
    }
    Some(total)
}

fn rank(request: SearchRequest) -> SearchResponse {
    let mut ranked = request
        .items
        .into_iter()
        .filter_map(|item| {
            let match_score = item_match_score(&item, &request.query)?;
            Some(RankedItem { item, match_score })
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
            .then_with(|| normalize(&left.item.title).cmp(&normalize(&right.item.title)))
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
    use super::*;

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
        assert_eq!(
            keys("chorme", vec![item("chrome", "Google Chrome")]),
            ["chrome"]
        );
        assert_eq!(
            keys("firwfox", vec![item("firefox", "Firefox")]),
            ["firefox"]
        );
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
