use std::str;

macro_rules! regex {
    ($re:literal $(,)?) => {{
        static RE: once_cell::sync::OnceCell<regex::Regex> = once_cell::sync::OnceCell::new();
        RE.get_or_init(|| regex::Regex::new($re).unwrap())
    }};
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    Other(String),
    Alpha(u64, Option<u64>),
    Beta(u64, Option<u64>),
    Ga(u64),
}

impl str::FromStr for Version {
    type Err = <u64 as str::FromStr>::Err;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let re = regex!(r"^v([\d]+)(?:(alpha|beta)([\d]*))?$");
        let version = if let Some(captures) = re.captures(text) {
            let major = captures
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .parse::<u64>()?;

            if let Some(alphabeta) = captures.get(2) {
                let minor = captures
                    .get(3)
                    .map(|m| m.as_str())
                    .unwrap_or_default()
                    .parse::<u64>()
                    .ok();

                if alphabeta.as_str() == "alpha" {
                    Self::Alpha(major, minor)
                } else {
                    Self::Beta(major, minor)
                }
            } else {
                Self::Ga(major)
            }
        } else {
            Self::Other(text.to_string())
        };

        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1() {
        let version: Version = "v1".parse().unwrap();
        assert_eq!(version, Version::Ga(1));
    }

    #[test]
    fn v3alpha2() {
        let version: Version = "v3alpha2".parse().unwrap();
        assert_eq!(version, Version::Alpha(3, Some(2)));
    }

    #[test]
    fn v2beta5() {
        let version: Version = "v2beta5".parse().unwrap();
        assert_eq!(version, Version::Beta(2, Some(5)));
    }

    #[test]
    fn v2beta() {
        let version: Version = "v2beta".parse().unwrap();
        assert_eq!(version, Version::Beta(2, None));
    }

    #[test]
    fn busted1() {
        let version: Version = "busted1".parse().unwrap();
        assert_eq!(version, Version::Other("busted1".to_string()));
    }

    #[test]
    fn v() {
        let version: Version = "v".parse().unwrap();
        assert_eq!(version, Version::Other("v".to_string()));
    }

    #[test]
    fn v1gamma2() {
        let version: Version = "v1gamma2".parse().unwrap();
        assert_eq!(version, Version::Other("v1gamma2".to_string()));
    }

    #[test]
    fn v1gamma() {
        let version: Version = "v1gamma".parse().unwrap();
        assert_eq!(version, Version::Other("v1gamma".to_string()));
    }

    #[test]
    fn compare() {
        assert!(Version::Ga(2) > Version::Ga(1));
        assert!(Version::Ga(1) > Version::Beta(2, None));
        assert!(Version::Ga(1) > Version::Beta(2, Some(2)));
        assert!(Version::Ga(1) > Version::Alpha(2, None));
        assert!(Version::Ga(1) > Version::Alpha(2, Some(3)));
        assert!(Version::Ga(1) > Version::Other("foo".to_string()));
        assert!(Version::Beta(1, Some(1)) > Version::Beta(1, None));
        assert!(Version::Beta(1, Some(2)) > Version::Beta(1, Some(1)));
        assert!(Version::Beta(1, None) > Version::Alpha(1, None));
        assert!(Version::Beta(1, None) > Version::Alpha(1, Some(3)));
        assert!(Version::Beta(1, None) > Version::Other("foo".to_string()));
        assert!(Version::Beta(1, Some(2)) > Version::Other("foo".to_string()));
        assert!(Version::Alpha(1, Some(1)) > Version::Alpha(1, None));
        assert!(Version::Alpha(1, Some(2)) > Version::Alpha(1, Some(1)));
        assert!(Version::Alpha(1, None) > Version::Other("foo".to_string()));
        assert!(Version::Alpha(1, Some(2)) > Version::Other("foo".to_string()));
        assert!(Version::Other("foo".to_string()) > Version::Other("bar".to_string()));
    }
}
