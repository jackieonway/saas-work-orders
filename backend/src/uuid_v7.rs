//! UUID policy shared by generation and every typed input boundary.

use uuid::{Uuid, Variant};

pub(crate) fn new() -> Uuid {
    Uuid::now_v7()
}

pub(crate) fn valid(id: Uuid) -> bool {
    id.get_version_num() == 7 && id.get_variant() == Variant::RFC4122
}

/// Accept the hyphenated wire form (case-insensitive), never compact/URN/nil/v4.
pub(crate) fn parse(raw: &str) -> Option<Uuid> {
    if raw.len() != 36 {
        return None;
    }
    let id = Uuid::parse_str(raw).ok()?;
    (valid(id) && id.hyphenated().to_string().eq_ignore_ascii_case(raw)).then_some(id)
}

#[cfg(test)]
pub(crate) fn fixture(value: u128) -> Uuid {
    // Fixed v7/RFC bits with a distinct low payload for deterministic test identities.
    Uuid::from_u128(0x01900000_0000_7000_8000_000000000000 | value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generator_and_strict_parser_share_the_same_policy() {
        let ids: HashSet<_> = (0..10_000).map(|_| new()).collect();
        assert_eq!(ids.len(), 10_000);
        for id in ids {
            assert!(valid(id));
            assert_eq!(parse(&id.to_string().to_uppercase()), Some(id));
            assert!(parse(&id.simple().to_string()).is_none());
        }
        // Explicit v4, nil and wrong-variant negatives are intentionally retained.
        for raw in [
            "01900000-0000-4000-8000-000000000001",
            "00000000-0000-0000-0000-000000000000",
            "01900000-0000-7000-c000-000000000001",
            "01900000-0000-7000-0000-000000000001",
        ] {
            assert!(parse(raw).is_none(), "{raw}");
        }
    }
}
