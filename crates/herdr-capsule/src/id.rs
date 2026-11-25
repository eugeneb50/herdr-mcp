use crate::{Result, Store};
use std::{collections::HashSet, fs, io::Read, time::SystemTime};
const CONSONANTS: &[u8] = b"kmrtslfh";
const VOWELS: &[u8] = b"aiou";
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
fn rand_bytes(n: usize) -> Vec<u8> {
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let mut buf = vec![0u8; n];
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    (0..n)
        .map(|i| {
            (seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(i as u64)
                >> 32) as u8
        })
        .collect()
}
pub fn mint(taken: &HashSet<String>, live: &HashSet<String>) -> String {
    let table = syllable_table();
    for n in 3..=8 {
        for _ in 0..2000 {
            let name: String = rand_bytes(n)
                .iter()
                .map(|b| table[*b as usize % table.len()].as_str())
                .collect();
            if !taken.contains(&name)
                && !WORDS.contains(&name.as_str())
                && !live.iter().any(|l| confusable(&name, l))
            {
                return name;
            }
        }
    }
    let mut name = String::new();
    for b in rand_bytes(4) {
        name.push_str(&table[b as usize % table.len()]);
    }
    name
}
pub fn mint_id(store: &mut Store) -> Result<String> {
    let taken: HashSet<String> = store.all_keys()?.into_iter().collect();
    Ok(mint(&taken, &HashSet::new()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mint_avoids_taken_and_real_words() {
        let taken = HashSet::from(["rotisu".to_string()]);
        let live = HashSet::from(["rotisu".to_string()]);
        let name = mint(&taken, &live);
        assert_eq!(name.len(), 6);
        assert!(!taken.contains(&name));
        assert!(!confusable(&name, "rotisu"));
        assert!(!WORDS.contains(&name.as_str()));
    }
}
