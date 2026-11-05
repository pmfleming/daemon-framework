use anyhow::{Context, Result};

/// A validated pointer selector, never an active-window or executable selector.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowAddress(String);

impl WindowAddress {
    pub fn parse(value: &str) -> Result<Self> {
        value
            .strip_prefix("0x")
            .and_then(Self::from_digits)
            .context("invalid window address")
    }

    // Event payloads carry bare pointers. Validate before allocating and share
    // the exact same bounds as public address selectors.
    pub(crate) fn from_digits(digits: &str) -> Option<Self> {
        ((1..=16).contains(&digits.len())
            && digits.bytes().all(|c| c.is_ascii_hexdigit())
            && digits.bytes().any(|c| c != b'0'))
        .then(|| {
            let mut value = format!("0x{digits}");
            value.make_ascii_lowercase();
            Self(value)
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WindowAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::WindowAddress;
    #[test]
    fn bounds_and_normalizes_pointer_selectors() {
        for value in ["0x1", "0xABC", "0xffffffffffffffff"] {
            assert!(WindowAddress::parse(value).is_ok());
        }
        assert_eq!(WindowAddress::parse("0xABC").unwrap().as_str(), "0xabc");
        for value in [
            "",
            "active",
            "0x",
            "0x0",
            "0x000",
            "0x10000000000000000",
            "0X1",
            "0xé",
            "0x1');evil()",
        ] {
            assert!(WindowAddress::parse(value).is_err(), "{value}");
        }
    }
}
