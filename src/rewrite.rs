//! Turns recognized text into what gets typed: voice commands ("new paragraph", "comma")
//! and the user's vocabulary ("cooper netties" -> "Kubernetes").
//!
//! The ASR streams text in pieces that may split words (" para", "graph"). Words that could
//! still become a phrase are held back until the following text decides; everything else
//! passes through right away.

use std::mem::take;

use crate::config::{Config, Replacement, VoiceCommand};

/// Punctuation the model puts after words. A punctuation command replaces it.
const PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', '…'];

#[derive(Debug)]
enum Action {
    /// Typed as a word.
    Write(String),
    /// Typed as Enter.
    Break(String),
    /// Attached to the previous word, replacing the model's punctuation there.
    Punctuate(String),
}

impl Action {
    /// A voice command's action, from what it types.
    fn command(output: &str) -> Self {
        if output.chars().all(|c| c == '\n') {
            Action::Break(output.to_owned())
        } else if output.chars().all(|c| PUNCTUATION.contains(&c)) {
            Action::Punctuate(output.to_owned())
        } else {
            Action::Write(output.to_owned())
        }
    }
}

/// Whether a voice command column applies to the dictation language: all of them while
/// detecting from speech.
fn applies(column: &str, language: &str) -> bool {
    let code = |l: &str| l.split('-').next().unwrap_or_default().to_ascii_lowercase();
    language == crate::language::DETECT || code(column) == code(language)
}

struct Rule {
    /// Normalized, see [`normalize`].
    words: Vec<String>,
    action: Action,
}

/// Rewrites one dictation. Feed the ASR pieces to [`push`](Self::push), then call
/// [`finish`](Self::finish) for the held-back rest.
pub struct Rewriter {
    rules: Vec<Rule>,
    /// Recognized text not rewritten yet.
    buffer: String,
    /// The buffer continues a word that was already passed on.
    mid_word: bool,
    writer: Writer,
}

impl Rewriter {
    /// `language` is the resolved language option, e.g. `"de-DE"` or `"auto"`.
    pub fn new(language: &str, settings: &Config) -> Self {
        let mut rules = Vec::new();
        if settings.voice_commands {
            let commands = settings.commands.iter().filter(|c| !c.output.is_empty());
            for VoiceCommand { output, phrases } in commands {
                let columns = phrases.iter().filter(|(column, _)| {
                    settings.command_languages.contains(column) && applies(column, language)
                });
                for phrase in columns.flat_map(|(_, phrases)| phrases.split(',')) {
                    rules.push(Rule {
                        words: words(phrase),
                        action: Action::command(output),
                    });
                }
            }
        }
        let vocabulary = settings.vocabulary.iter().filter(|r| !r.written.trim().is_empty());
        for Replacement { written, heard } in vocabulary {
            for heard in heard {
                rules.push(Rule {
                    words: words(heard),
                    action: Action::Write(written.trim().to_owned()),
                });
            }
        }
        rules.retain(|rule| !rule.words.is_empty());
        Self {
            rules,
            buffer: String::new(),
            mid_word: false,
            writer: Writer::default(),
        }
    }

    /// Takes the next ASR piece and returns the text that is ready to type.
    pub fn push(&mut self, piece: &str) -> String {
        if self.rules.is_empty() {
            return piece.to_owned();
        }
        self.buffer.push_str(piece);
        self.process(false)
    }

    /// Returns the text held back at the end of the dictation.
    pub fn finish(&mut self) -> String {
        let mut out = self.process(true);
        self.writer.finish(&mut out);
        self.buffer.clear();
        out
    }

    /// Rewrites a complete transcript.
    pub fn apply(mut self, text: &str) -> String {
        let mut out = self.push(text);
        out.push_str(&self.finish());
        out
    }

    fn process(&mut self, last: bool) -> String {
        let buffer = take(&mut self.buffer);
        let tokens = tokenize(&buffer, last);
        let mut out = String::new();
        let mut consumed = 0;
        let mut i = 0;
        while let Some(token) = tokens.get(i) {
            let mut used = 1;
            if i == 0 && self.mid_word && token.space.is_empty() {
                self.writer.append(&mut out, token.word);
            } else {
                match self.find(&tokens[i..], last) {
                    Found::Wait => break,
                    Found::Nothing => self.writer.word(&mut out, token.space, token.word, true),
                    Found::Rule(rule) => {
                        let rule = &self.rules[rule];
                        used = rule.words.len();
                        self.writer.rule(&mut out, &tokens[i..i + used], &rule.action);
                    }
                }
            }
            i += used;
            self.mid_word = !tokens[i - 1].complete;
            consumed = tokens[i - 1].end;
        }
        self.buffer = buffer[consumed..].to_owned();
        out
    }

    /// The longest rule at the start of `tokens`, unless one may still match.
    fn find(&self, tokens: &[Token], last: bool) -> Found {
        let mut best: Option<usize> = None;
        for (index, rule) in self.rules.iter().enumerate() {
            match matches(&rule.words, tokens, last) {
                Match::Maybe => return Found::Wait,
                Match::Yes => {
                    if best.is_none_or(|b| rule.words.len() > self.rules[b].words.len()) {
                        best = Some(index);
                    }
                }
                Match::No => {}
            }
        }
        best.map_or(Found::Nothing, Found::Rule)
    }
}

enum Found {
    /// More text is needed to decide.
    Wait,
    Nothing,
    Rule(usize),
}

enum Match {
    Yes,
    No,
    Maybe,
}

fn matches(words: &[String], tokens: &[Token], last: bool) -> Match {
    for (k, word) in words.iter().enumerate() {
        let Some(token) = tokens.get(k) else {
            return if last { Match::No } else { Match::Maybe };
        };
        let heard = normalize(token.word);
        if !token.complete {
            return if word.starts_with(&heard) { Match::Maybe } else { Match::No };
        }
        if heard != *word {
            return Match::No;
        }
    }
    Match::Yes
}

/// Output state carried across pieces.
#[derive(Default)]
struct Writer {
    /// Punctuation at the end of the output so far, held back in case a punctuation
    /// command replaces it.
    punctuation: String,
    /// Drop the space before the next word (after a line break).
    skip_space: bool,
    /// Capitalize the next word (after a line break or sentence end).
    capitalize: bool,
}

impl Writer {
    fn word(&mut self, out: &mut String, space: &str, word: &str, capitalize: bool) {
        out.push_str(&take(&mut self.punctuation));
        if !take(&mut self.skip_space) {
            out.push_str(space);
        }
        if take(&mut self.capitalize) && capitalize {
            self.append(out, &capitalized(word));
        } else {
            self.append(out, word);
        }
    }

    /// Continues the last word.
    fn append(&mut self, out: &mut String, text: &str) {
        let text = take(&mut self.punctuation) + text;
        let body = text.trim_end_matches(PUNCTUATION);
        out.push_str(body);
        self.punctuation = text[body.len()..].to_owned();
    }

    fn rule(&mut self, out: &mut String, tokens: &[Token], action: &Action) {
        match action {
            Action::Write(written) => {
                let first = tokens[0].word;
                let last = tokens[tokens.len() - 1].word;
                let before = &first[..first.len() - first.trim_start_matches(not_alphanumeric).len()];
                let after = &last[last.trim_end_matches(not_alphanumeric).len()..];
                self.word(out, tokens[0].space, &format!("{before}{written}{after}"), false);
            }
            // Whatever the model put around the command words is dropped.
            Action::Break(text) => {
                out.push_str(&take(&mut self.punctuation));
                out.push_str(text);
                self.skip_space = true;
                self.capitalize = true;
            }
            Action::Punctuate(mark) => {
                self.punctuation.clear();
                out.push_str(mark);
                self.capitalize = mark.ends_with(['.', '?', '!']);
            }
        }
    }

    fn finish(&mut self, out: &mut String) {
        out.push_str(&take(&mut self.punctuation));
    }
}

struct Token<'a> {
    /// Whitespace before the word.
    space: &'a str,
    word: &'a str,
    /// Byte offset after the word.
    end: usize,
    /// Followed by whitespace, or the text is complete. Otherwise the word may go on.
    complete: bool,
}

fn tokenize(text: &str, last: bool) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut start = 0;
    while let Some(offset) = text[start..].find(|c: char| !c.is_whitespace()) {
        let word_start = start + offset;
        let end = text[word_start..]
            .find(char::is_whitespace)
            .map_or(text.len(), |len| word_start + len);
        tokens.push(Token {
            space: &text[start..word_start],
            word: &text[word_start..end],
            end,
            complete: last || end < text.len(),
        });
        start = end;
    }
    tokens
}

fn words(phrase: &str) -> Vec<String> {
    phrase
        .split_whitespace()
        .map(normalize)
        .filter(|word| !word.is_empty())
        .collect()
}

/// For matching: lowercase, without punctuation around the word.
fn normalize(word: &str) -> String {
    word.trim_matches(not_alphanumeric).to_lowercase()
}

fn not_alphanumeric(c: char) -> bool {
    !c.is_alphanumeric()
}

fn capitalized(word: &str) -> String {
    match word.char_indices().find(|(_, c)| c.is_alphanumeric()) {
        Some((i, c)) if c.is_lowercase() => {
            format!("{}{}{}", &word[..i], c.to_uppercase(), &word[i + c.len_utf8()..])
        }
        _ => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `vocabulary`: written, heard.
    fn rewriter(language: &str, vocabulary: &[(&str, &[&str])]) -> Rewriter {
        let settings = Config {
            vocabulary: vocabulary
                .iter()
                .map(|(written, heard)| Replacement {
                    written: written.to_string(),
                    heard: heard.iter().map(|h| h.to_string()).collect(),
                })
                .collect(),
            ..Config::default()
        };
        Rewriter::new(language, &settings)
    }

    /// Rewrites `text` as a whole and split into pieces at every possible position,
    /// and checks that all give `expected`.
    fn check(language: &str, vocabulary: &[(&str, &[&str])], text: &str, expected: &str) {
        assert_eq!(rewriter(language, vocabulary).apply(text), expected, "whole");
        let cuts: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).collect();
        for &a in &cuts {
            let mut r = rewriter(language, vocabulary);
            let out = r.push(&text[..a]) + &r.push(&text[a..]) + &r.finish();
            assert_eq!(out, expected, "split at {a}");
        }
        let mut r = rewriter(language, vocabulary);
        let mut out = String::new();
        for c in text.chars() {
            out += &r.push(&c.to_string());
        }
        out += &r.finish();
        assert_eq!(out, expected, "char by char");
    }

    #[test]
    fn plain_text_passes_through() {
        check("en", &[], " Hello world. How are you?", " Hello world. How are you?");
        check("de", &[], " Das kostet 3.5 Euro, ok", " Das kostet 3.5 Euro, ok");
    }

    #[test]
    fn line_breaks() {
        check("en", &[], " Hello world. New paragraph. This is it", " Hello world.\n\nThis is it");
        check("en", &[], " one new line two", " one\nTwo");
        check("de", &[], " Hallo, neue Zeile, wie geht's", " Hallo,\nWie geht's");
        check("en", &[], " a new line", " a\n");
    }

    #[test]
    fn punctuation_replaces_the_models() {
        check("en", &[], " Hello world, full stop. this", " Hello world. This");
        check("en", &[], " Hello comma world", " Hello, world");
        check("en", &[], " Really question mark", " Really?");
        check("de", &[], " Hallo Welt Punkt", " Hallo Welt.");
        check("de", &[], " Wirklich Fragezeichen Ausrufezeichen", " Wirklich?!");
    }

    #[test]
    fn similar_words_are_left_alone() {
        check("en", &[], " the newest lines, commas", " the newest lines, commas");
        check("en", &[], " a new linear model", " a new linear model");
        check("en", &[], " new", " new");
    }

    #[test]
    fn commands_follow_the_language() {
        check("en", &[], " Punkt", " Punkt");
        check("de", &[], " comma", " comma");
        check("auto", &[], " Hallo Komma hello comma", " Hallo, hello,");
        check("fr", &[], " comma", " comma");
    }

    #[test]
    fn vocabulary() {
        let vocabulary: [(&str, &[&str]); 3] = [
            ("Kubernetes", &["cooper netties", "cube netties"]),
            ("Freisprech", &["free sprech"]),
            ("", &["ignored"]),
        ];
        check("en", &vocabulary, " I use Cooper Netties.", " I use Kubernetes.");
        check("en", &vocabulary, " (cube netties) and free sprech", " (Kubernetes) and Freisprech");
        check("en", &vocabulary, " cooper nettiesx", " cooper nettiesx");
        check("en", &vocabulary, " free sprech, comma", " Freisprech,");
        check("en", &vocabulary, " ignored", " ignored");
    }

    #[test]
    fn custom_commands() {
        let mut settings = Config::default();
        settings.commands.retain(|c| c.output != ",");
        settings.commands.push(VoiceCommand {
            output: "–".into(),
            phrases: [("de".into(), "Gedankenstrich".into())].into(),
        });
        settings.commands.push(VoiceCommand {
            output: ".".into(),
            phrases: [("fr".into(), "point final".into())].into(),
        });
        // Not typed until it has an output.
        settings.commands.push(VoiceCommand {
            output: String::new(),
            phrases: [("de".into(), "sehr".into())].into(),
        });
        let german = " gut Gedankenstrich sehr gut Komma";
        assert_eq!(Rewriter::new("de", &settings).apply(german), " gut – sehr gut Komma");
        let french = " bonjour point final merci";
        // Without a column for the language, its phrases don't count.
        assert_eq!(Rewriter::new("fr-FR", &settings).apply(french), french);
        settings.command_languages.push("fr".into());
        assert_eq!(Rewriter::new("fr-FR", &settings).apply(french), " bonjour. Merci");
    }

    #[test]
    fn disabled_commands() {
        let settings = Config {
            voice_commands: false,
            ..Config::default()
        };
        let text = " Hello comma world new line";
        assert_eq!(Rewriter::new("en", &settings).apply(text), text);
    }
}
