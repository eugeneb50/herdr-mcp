use crate::Result;
use std::{collections::HashSet, fs, io::Read};
const CONSONANTS: &[u8] = b"kmrtslfh";
const VOWELS: &[u8] = b"aiou";
/// Capsule ids have this many syllables: 32^4 ≈ 1M ids, eight letters an agent
/// can retype. The queue reserves each one (`Queue::push_unique`).
const ID_SYLLABLES: usize = 4;
const WORDS: &[&str] = &[
    "kimono", "salami", "tomato", "lasso", "solo", "tofu", "haiku", "hula", "koala", "mama",
    "momo", "lima", "sumo", "tutu", "aloha", "kahuna", "karma", "kilo", "silo", "halo", "loft",
    "taro", "tiramisu", "sofa", "hotfix", "toto", "lulu", "kiki", "fifa", "mafia", "sushi", "tiki",
];
fn syllable_table() -> Vec<String> {
    CONSONANTS
        .iter()
        .flat_map(|c| {
            VOWELS
                .iter()
                .map(move |v| format!("{}{}", *c as char, *v as char))
        })
        .collect()
}
pub fn syllables(name: &str) -> Vec<String> {
    name.chars()
        .collect::<Vec<_>>()
        .chunks(2)
        .map(|c| c.iter().collect())
        .collect()
}
pub fn confusable(a: &str, b: &str) -> bool {
    let (sa, sb) = (syllables(a), syllables(b));
    sa.len() == sb.len() && sa.iter().zip(&sb).filter(|(x, y)| x != y).count() <= 1
}
fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}
/// `bytes` as syllables. 256 is a multiple of the 32-entry table, so every
/// syllable is equally likely.
fn spell(bytes: &[u8]) -> String {
    let table = syllable_table();
    bytes
        .iter()
        .map(|b| table[*b as usize % table.len()].as_str())
        .collect()
}
/// A callsign: not taken, not a word, not one letter away from a live one.
pub fn mint(taken: &HashSet<String>, live: &HashSet<String>) -> Result<String> {
    for n in 3..=8 {
        for _ in 0..2000 {
            let name = spell(&random_bytes(n)?);
            if !taken.contains(&name)
                && !WORDS.contains(&name.as_str())
                && !live.iter().any(|l| confusable(&name, l))
            {
                return Ok(name);
            }
        }
    }
    Err(crate::fail("no free callsign after 12000 tries"))
}
/// A candidate capsule id. Uniqueness is the queue's job: `push_unique`
/// refuses a taken one and the caller draws again.
pub fn capsule_id() -> Result<String> {
    loop {
        let id = spell(&random_bytes(ID_SYLLABLES)?);
        if !WORDS.contains(&id.as_str()) {
            return Ok(id);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mint_avoids_taken_and_real_words() {
        let taken = HashSet::from(["rotisu".to_string()]);
        let live = HashSet::from(["rotisu".to_string()]);
        let name = mint(&taken, &live).unwrap();
        assert!(!taken.contains(&name));
        assert!(!confusable(&name, "rotisu"));
        assert!(!WORDS.contains(&name.as_str()));
    }
    #[test]
    fn capsule_ids_are_four_syllables_of_the_table() {
        let table = syllable_table();
        for _ in 0..200 {
            let id = capsule_id().unwrap();
            assert_eq!(id.len(), 2 * ID_SYLLABLES, "{id}");
            assert!(syllables(&id).iter().all(|s| table.contains(s)), "{id}");
        }
    }
    #[test]
    fn every_byte_value_spells_a_syllable() {
        let all: Vec<u8> = (0..=255).collect();
        let spelled = spell(&all);
        assert_eq!(spelled.len(), 512);
        // Each of the 32 syllables appears exactly 8 times: no bias.
        let counts = syllables(&spelled).into_iter().fold(
            std::collections::HashMap::<String, usize>::new(),
            |mut m, s| {
                *m.entry(s).or_default() += 1;
                m
            },
        );
        assert_eq!(counts.len(), 32);
        assert!(counts.values().all(|&n| n == 8), "{counts:?}");
    }
}
