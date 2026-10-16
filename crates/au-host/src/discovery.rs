//! Manufacturer query policy, kept independent of AudioToolbox for cross-platform tests.

use std::ffi::OsStr;

pub(crate) const MANUFACTURERS_ENV: &str = "SOUNDCRAFT_AU_MANUFACTURERS";
const MAX_MANUFACTURERS: usize = 64;

/// None preserves the system-wide query; an empty list performs no enumeration.
pub(crate) struct DiscoveryPolicy {
    manufacturers: Option<Vec<u32>>,
}

impl DiscoveryPolicy {
    /// Codes are comma-separated, exactly four printable ASCII bytes, including padded spaces.
    /// At most 64 distinct codes are accepted; malformed or non-Unicode overrides disable all queries.
    pub(crate) fn from_override(value: Option<&OsStr>) -> Self {
        let Some(value) = value else { return Self { manufacturers: None } };
        let disabled = || Self { manufacturers: Some(Vec::new()) };
        let Some(value) = value.to_str() else { return disabled() };
        if value.is_empty() {
            return disabled();
        }
        let mut manufacturers = Vec::new();
        for token in value.split(',') {
            let Ok(bytes) = <[u8; 4]>::try_from(token.as_bytes()) else { return disabled() };
            if !bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
                return disabled();
            }
            let manufacturer = u32::from_be_bytes(bytes);
            if !manufacturers.contains(&manufacturer) {
                if manufacturers.len() == MAX_MANUFACTURERS || manufacturers.try_reserve(1).is_err() {
                    return disabled();
                }
                manufacturers.push(manufacturer);
            }
        }
        Self { manufacturers: Some(manufacturers) }
    }

    /// Dispatch only admitted queries; filtering happens before the component manager is called.
    pub(crate) fn for_each_query(&self, mut query: impl FnMut(u32, Option<u32>)) {
        for ty in crate::SUPPORTED_TYPES {
            match &self.manufacturers {
                None => query(ty, None),
                Some(manufacturers) => {
                    for &manufacturer in manufacturers {
                        query(ty, Some(manufacturer));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLE: u32 = u32::from_be_bytes(*b"appl");
    const OTHER: u32 = u32::from_be_bytes(*b"TEST");

    fn queries(value: Option<&OsStr>) -> Vec<(u32, Option<u32>)> {
        let mut queries = Vec::new();
        DiscoveryPolicy::from_override(value).for_each_query(|ty, manufacturer| queries.push((ty, manufacturer)));
        queries
    }

    fn simulated_scan(value: Option<&OsStr>) -> (Vec<(u32, Option<u32>)>, Vec<(u32, u32)>) {
        let mut queries = Vec::new();
        let mut inspected = Vec::new();
        DiscoveryPolicy::from_override(value).for_each_query(|ty, manufacturer| {
            queries.push((ty, manufacturer));
            // Model the component manager's manufacturer query before metadata inspection.
            for candidate in [APPLE, OTHER] {
                if manufacturer.is_none_or(|requested| requested == candidate) {
                    inspected.push((ty, candidate));
                }
            }
        });
        (queries, inspected)
    }

    #[test]
    fn unset_preserves_each_legacy_wildcard_query() {
        assert_eq!(MANUFACTURERS_ENV, "SOUNDCRAFT_AU_MANUFACTURERS");
        let (queries, inspected) = simulated_scan(None);
        assert_eq!(queries, crate::SUPPORTED_TYPES.map(|ty| (ty, None)));
        assert_eq!(inspected.len(), 2 * crate::SUPPORTED_TYPES.len());
        assert!(inspected.iter().any(|(_, manufacturer)| *manufacturer == OTHER));
    }

    #[test]
    fn apple_override_never_queries_or_inspects_other_manufacturers() {
        let (queries, inspected) = simulated_scan(Some(OsStr::new("appl")));
        assert_eq!(queries, crate::SUPPORTED_TYPES.map(|ty| (ty, Some(APPLE))));
        assert_eq!(inspected, crate::SUPPORTED_TYPES.map(|ty| (ty, APPLE)));
    }

    #[test]
    fn empty_override_makes_no_component_manager_calls() {
        let (queries, inspected) = simulated_scan(Some(OsStr::new("")));
        assert!(queries.is_empty());
        assert!(inspected.is_empty());
    }

    #[test]
    fn malformed_tokens_disable_the_whole_override_without_fallback() {
        for value in
            ["app", "applx", "appl,", ",appl", "appl,,TEST", "appl,TEST,", "appl, TEST", "appl,éabc", "appl,\0abc", "appl,\nabc", "#6170706C"]
        {
            let (queries, inspected) = simulated_scan(Some(OsStr::new(value)));
            assert!(queries.is_empty(), "invalid override {value:?} made a query");
            assert!(inspected.is_empty());
        }
    }

    #[test]
    fn multiple_codes_are_case_sensitive_and_deduplicated() {
        let actual = queries(Some(OsStr::new("appl,TEST,appl,test")));
        let expected: Vec<_> = crate::SUPPORTED_TYPES
            .into_iter()
            .flat_map(|ty| [APPLE, OTHER, u32::from_be_bytes(*b"test")].map(|manufacturer| (ty, Some(manufacturer))))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn four_byte_printable_codes_preserve_padding() {
        assert_eq!(queries(Some(OsStr::new("Ni  "))), crate::SUPPORTED_TYPES.map(|ty| (ty, Some(u32::from_be_bytes(*b"Ni  ")))));
        assert_eq!(queries(Some(OsStr::new("AB# "))), crate::SUPPORTED_TYPES.map(|ty| (ty, Some(u32::from_be_bytes(*b"AB# ")))));
    }

    #[test]
    fn manufacturer_list_limit_is_bounded_and_fails_closed() {
        let values: Vec<_> = (0..MAX_MANUFACTURERS).map(|index| format!("{index:04}")).collect();
        let at_limit = values.join(",");
        assert_eq!(queries(Some(OsStr::new(&at_limit))).len(), MAX_MANUFACTURERS * crate::SUPPORTED_TYPES.len());
        let over_limit = format!("{at_limit},0064");
        assert!(queries(Some(OsStr::new(&over_limit))).is_empty());
        let duplicate_over_limit = vec!["appl"; MAX_MANUFACTURERS + 1].join(",");
        assert_eq!(queries(Some(OsStr::new(&duplicate_over_limit))), crate::SUPPORTED_TYPES.map(|ty| (ty, Some(APPLE))));
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_override_makes_no_queries() {
        use std::os::unix::ffi::OsStrExt;
        let (queries, inspected) = simulated_scan(Some(OsStr::from_bytes(b"appl,\xffabc")));
        assert!(queries.is_empty());
        assert!(inspected.is_empty());
    }
}
