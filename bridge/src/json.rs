//! JSON, as the bot API speaks it (`bots`): enough to read a bot's requests and write what the
//! bridge tells it. Written here rather than taken from a crate, as the bridge keeps its
//! dependencies few; it reads any JSON, of no more than `MAX_DEPTH` levels.

use std::fmt::{self, Display, Write as _};

/// The deepest a value may nest: a request is a flat object, and anything far deeper is not
/// one, and would only cost the stack.
const MAX_DEPTH: usize = 32;

/// A JSON value. An object keeps its members in order, as they were read or added.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// An object of `members`, in their order.
    #[must_use]
    pub fn object<const N: usize>(members: [(&str, Value); N]) -> Self {
        Self::Object(
            members
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    /// The member `key` of an object; None for another value, or an object without it.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Adds a member to an object; to anything else, nothing.
    pub fn insert(&mut self, key: &str, value: Self) {
        if let Self::Object(members) = self {
            members.push((key.to_owned(), value));
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::String(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::String(text)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<f64> for Value {
    fn from(number: f64) -> Self {
        Self::Number(number)
    }
}

impl From<f32> for Value {
    /// By the shortest decimal that is the float, so 0.1 stays 0.1 and is not written as the
    /// double nearest the float.
    fn from(number: f32) -> Self {
        Self::Number(number.to_string().parse().unwrap_or(f64::NAN))
    }
}

macro_rules! from_integer {
    ($($integer:ty),*) => {
        $(impl From<$integer> for Value {
            fn from(number: $integer) -> Self {
                Self::Number(f64::from(number))
            }
        })*
    };
}

from_integer!(i8, u8, i16, u16, i32, u32);

impl<T: Into<Self>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

impl<T: Into<Self>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Self::Array(items.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Self>, const N: usize> From<[T; N]> for Value {
    fn from(items: [T; N]) -> Self {
        Self::Array(items.into_iter().map(Into::into).collect())
    }
}

fn write_string(out: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    out.write_char('"')?;
    for character in text.chars() {
        match character {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            control if u32::from(control) < 0x20 => write!(out, "\\u{:04x}", u32::from(control))?,
            other => out.write_char(other)?,
        }
    }
    out.write_char('"')
}

impl Display for Value {
    /// The value as compact JSON, on one line: a number that is no number is `null`.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(out, "{value}"),
            Self::Number(number) if number.is_finite() => write!(out, "{number}"),
            Self::Null | Self::Number(_) => out.write_str("null"),
            Self::String(text) => write_string(out, text),
            Self::Array(items) => {
                out.write_char('[')?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.write_char(',')?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_char(']')
            }
            Self::Object(members) => {
                out.write_char('{')?;
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.write_char(',')?;
                    }
                    write_string(out, key)?;
                    write!(out, ":{value}")?;
                }
                out.write_char('}')
            }
        }
    }
}

/// Reads `text` as one JSON value, with nothing but white space around it.
///
/// # Errors
/// What is wrong with it, in words, if it is not JSON or nests too deep.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        at: 0,
    };
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.at == reader.bytes.len() {
        Ok(value)
    } else {
        Err(reader.wrong("more after the value"))
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn wrong(&self, what: &str) -> String {
        format!("not JSON: {what} at character {}", self.at + 1)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| b" \t\r\n".contains(&byte)) {
            self.at += 1;
        }
    }

    fn expect(&mut self, word: &str) -> Result<(), String> {
        if self.bytes.get(self.at..self.at + word.len()) == Some(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(self.wrong("an unknown word"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.wrong("too deep"));
        }
        self.skip_space();
        match self.peek() {
            Some(b'n') => self.expect("null").map(|()| Value::Null),
            Some(b't') => self.expect("true").map(|()| Value::Bool(true)),
            Some(b'f') => self.expect("false").map(|()| Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.wrong("an unexpected character")),
            None => Err(self.wrong("the end")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.wrong("no , or ] in an array")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.at += 1;
        let mut members = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return Err(self.wrong("a member with no name"));
            }
            let key = self.string()?;
            self.skip_space();
            if self.peek() != Some(b':') {
                return Err(self.wrong("no : after a member's name"));
            }
            self.at += 1;
            members.push((key, self.value(depth + 1)?));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(self.wrong("no , or } in an object")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_digit() || b"+-.eE".contains(&byte))
        {
            self.at += 1;
        }
        std::str::from_utf8(self.bytes.get(start..self.at).unwrap_or_default())
            .ok()
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|number| number.is_finite())
            .map(Value::Number)
            .ok_or_else(|| self.wrong("a number that is not one"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.bytes.get(self.at..self.at + 4).unwrap_or_default();
        let unit = std::str::from_utf8(digits)
            .ok()
            .filter(|_| digits.len() == 4)
            .and_then(|text| u32::from_str_radix(text, 16).ok())
            .ok_or_else(|| self.wrong("a \\u with no four hex digits"))?;
        self.at += 4;
        Ok(unit)
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut text = String::new();
        loop {
            let start = self.at;
            while self
                .peek()
                .is_some_and(|byte| byte != b'"' && byte != b'\\' && byte >= 0x20)
            {
                self.at += 1;
            }
            let run = self.bytes.get(start..self.at).unwrap_or_default();
            text.push_str(
                std::str::from_utf8(run).map_err(|_| self.wrong("text that is not UTF-8"))?,
            );
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(text);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escaped = self.peek().ok_or_else(|| self.wrong("the end"))?;
                    self.at += 1;
                    let character = match escaped {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => return Err(self.wrong("an unknown escape")),
                    };
                    text.push(character);
                }
                _ => return Err(self.wrong("a string that does not end")),
            }
        }
    }

    /// The character of a `\u` escape, whose `\u` is read, with the second half of a pair.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let first = self.hex4()?;
        let unit = if (0xD800..0xDC00).contains(&first) {
            if self.bytes.get(self.at..self.at + 2) != Some(b"\\u") {
                return Err(self.wrong("half of a pair of \\u"));
            }
            self.at += 2;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(self.wrong("half of a pair of \\u"));
            }
            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
        } else {
            first
        };
        char::from_u32(unit).ok_or_else(|| self.wrong("a \\u that is no character"))
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_DEPTH, Value, parse};

    #[test]
    fn a_request_is_read_with_every_kind_of_value() -> Result<(), String> {
        let read = parse(
            r#" {"type":"pad","buttons":["A","Z"],"stick":[-0.5,1e0],"frames":30,"x":null,"on":true} "#,
        )?;
        assert_eq!(read.get("type").and_then(Value::as_str), Some("pad"));
        let buttons = read
            .get("buttons")
            .and_then(Value::as_array)
            .unwrap_or_default();
        assert_eq!(buttons.len(), 2);
        let stick: Vec<f64> = read
            .get("stick")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        assert_eq!(stick, [-0.5, 1.0]);
        assert_eq!(read.get("frames").and_then(Value::as_f64), Some(30.0));
        assert_eq!(read.get("x"), Some(&Value::Null));
        assert_eq!(read.get("on").and_then(Value::as_bool), Some(true));
        assert_eq!(read.get("missing"), None);
        Ok(())
    }

    #[test]
    fn text_with_escapes_reads_back_as_it_was_written() -> Result<(), String> {
        let text = "a \"quote\", a \\ and\na line, \u{1} and \u{e9} and \u{1f600}";
        let written = Value::object([("say", text.into())]).to_string();
        assert!(!written.contains('\n'));
        assert_eq!(
            parse(&written)?.get("say").and_then(Value::as_str),
            Some(text)
        );
        assert_eq!(
            parse(r#""\u00e9\ud83d\ude00\/""#)?,
            Value::from("\u{e9}\u{1f600}/")
        );
        Ok(())
    }

    #[test]
    fn numbers_are_written_short_and_what_is_no_number_as_null() {
        let value = Value::object([
            ("whole", 3_u8.into()),
            ("float", 0.1_f32.into()),
            ("nan", f32::NAN.into()),
            ("none", Option::<u8>::None.into()),
            ("list", [1.5_f32, -2.0].into()),
        ]);
        assert_eq!(
            value.to_string(),
            r#"{"whole":3,"float":0.1,"nan":null,"none":null,"list":[1.5,-2]}"#
        );
    }

    #[test]
    fn what_is_not_json_is_refused_and_says_where() {
        for wrong in [
            "",
            "{",
            "{\"a\" 1}",
            "[1,]",
            "{\"a\":1}x",
            "tru",
            "\"never ends",
            "\"\\x\"",
            "\"\\ud83d\"",
            "1e999",
            "POST / HTTP/1.1",
        ] {
            assert!(parse(wrong).is_err(), "{wrong}");
        }
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(parse(&deep).is_err());
        let fine = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&fine).is_ok());
    }
}
