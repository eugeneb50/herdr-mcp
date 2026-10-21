//! A small, language-agnostic code highlighter for the send box: comments
//! (`//`, `#`), strings, numbers, CLI flags and common keywords of shell,
//! Rust, Python and JS. Tokens concatenate back to the input exactly.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Comment,
    Str,
    Num,
    Keyword,
    Flag,
}

impl Kind {
    pub fn class(self) -> &'static str {
        match self {
            Kind::Plain => "",
            Kind::Comment => "hl-comment",
            Kind::Str => "hl-str",
            Kind::Num => "hl-num",
            Kind::Keyword => "hl-kw",
            Kind::Flag => "hl-flag",
        }
    }
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "case", "cd", "class", "const", "continue", "def", "do",
    "done", "echo", "elif", "else", "enum", "esac", "export", "false", "fi", "fn", "for", "from",
    "function", "if", "impl", "import", "in", "let", "loop", "match", "mod", "mut", "None", "null",
    "pub", "return", "self", "Self", "struct", "then", "trait", "true", "type", "use", "var",
    "where", "while",
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Split `src` into highlighted runs; adjacent plain text is merged.
pub fn tokens(src: &str) -> Vec<(Kind, &str)> {
    let mut runs: Vec<(Kind, usize, usize)> = Vec::new();
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let end_of = |i: usize| chars.get(i).map_or(src.len(), |&(b, _)| b);
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        let prev = i.checked_sub(1).map(|p| chars[p].1);
        let after_space = prev.is_none_or(char::is_whitespace);
        let next = chars.get(i + 1).map(|&(_, c)| c);
        let mut j = i + 1;
        let kind = if (c == '#' && after_space) || (c == '/' && next == Some('/')) {
            while j < chars.len() && chars[j].1 != '\n' {
                j += 1;
            }
            Kind::Comment
        } else if c == '"' || c == '`' || (c == '\'' && !prev.is_some_and(is_word)) {
            // `"` and `'` close at the line end; backticks may span lines.
            while j < chars.len() && chars[j].1 != c && !(c != '`' && chars[j].1 == '\n') {
                j += if chars[j].1 == '\\' { 2 } else { 1 };
            }
            j = (j + 1).min(chars.len());
            Kind::Str
        } else if c == '-' && after_space && next.is_some_and(|n| n == '-' || n.is_alphabetic()) {
            while j < chars.len() && (is_word(chars[j].1) || chars[j].1 == '-') {
                j += 1;
            }
            Kind::Flag
        } else if is_word(c) && !prev.is_some_and(is_word) {
            while j < chars.len()
                && (is_word(chars[j].1) || (chars[j].1 == '.' && c.is_ascii_digit()))
            {
                j += 1;
            }
            let word = &src[start..end_of(j)];
            if c.is_ascii_digit() {
                Kind::Num
            } else if KEYWORDS.contains(&word) {
                Kind::Keyword
            } else {
                Kind::Plain
            }
        } else {
            Kind::Plain
        };
        match runs.last_mut() {
            Some((Kind::Plain, _, end)) if kind == Kind::Plain => *end = end_of(j),
            _ => runs.push((kind, start, end_of(j))),
        }
        i = j;
    }
    runs.into_iter()
        .map(|(kind, a, b)| (kind, &src[a..b]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, &str)> {
        tokens(src)
            .into_iter()
            .filter(|(k, _)| *k != Kind::Plain)
            .collect()
    }

    #[test]
    fn round_trips() {
        for src in [
            "",
            "don't stop",
            "a \"b\\\"c\" 'd",
            "x // y\n# z\n`a\nb`",
            "é 1.5 --fix ✓",
        ] {
            assert_eq!(tokens(src).iter().map(|(_, s)| *s).collect::<String>(), src);
        }
    }

    #[test]
    fn classifies() {
        use Kind::*;
        assert_eq!(
            kinds("let x = 42; // done\ncargo test --workspace -p ui 'a b' # note"),
            vec![
                (Keyword, "let"),
                (Num, "42"),
                (Comment, "// done"),
                (Flag, "--workspace"),
                (Flag, "-p"),
                (Str, "'a b'"),
                (Comment, "# note"),
            ]
        );
        // Prose stays plain: apostrophes, mid-word `#`, hyphenated words.
        assert_eq!(kinds("don't use C# re-run"), vec![(Keyword, "use")]);
    }
}
