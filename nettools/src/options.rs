//! Command-line parsing shared by the tools: positional arguments and `--name value` options.

use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug)]
pub struct Options {
    pub positional: Vec<String>,
    named: HashMap<String, String>,
}

impl Options {
    /// Splits `args`, accepting only the options in `names`.
    ///
    /// # Errors
    /// Fails on an unknown option or one without a value.
    pub fn parse(args: impl IntoIterator<Item = String>, names: &[&str]) -> Result<Self, String> {
        let mut options = Self {
            positional: Vec::new(),
            named: HashMap::new(),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let Some(name) = arg.strip_prefix("--") else {
                options.positional.push(arg);
                continue;
            };
            if !names.contains(&name) {
                return Err(format!("unknown option --{name}"));
            }
            let value = args.next().ok_or(format!("--{name} needs a value"))?;
            options.named.insert(name.to_owned(), value);
        }
        Ok(options)
    }

    /// The value of `--name`, or `default` when it was not given.
    ///
    /// # Errors
    /// Fails if the value does not parse.
    pub fn get<T: FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        self.named.get(name).map_or(Ok(default), |value| {
            value
                .parse()
                .map_err(|_| format!("--{name} {value} is not a valid value"))
        })
    }

    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.named.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::Options;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(ToString::to_string), &["delay", "loss"])
    }

    #[test]
    fn positionals_and_options_mix_in_any_order() -> Result<(), String> {
        let options = parse(&["--delay", "40", "a", "b", "--loss", "2.5"])?;
        assert_eq!(options.positional, ["a", "b"]);
        assert_eq!(options.get("delay", 0_u64)?, 40);
        assert!((options.get("loss", 0.0_f64)? - 2.5).abs() < 1e-9);
        assert!(!options.has("jitter"));
        Ok(())
    }

    #[test]
    fn defaults_fill_in_and_mistakes_are_reported() -> Result<(), String> {
        assert_eq!(parse(&[])?.get("delay", 7_u64)?, 7);
        assert!(parse(&["--jitter", "5"]).is_err());
        assert!(parse(&["--delay"]).is_err());
        assert!(parse(&["--delay", "soon"])?.get("delay", 0_u64).is_err());
        Ok(())
    }
}
