use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    entries: BTreeSet<String>,
    wildcard: bool,
}

pub fn normalize(raw: &str) -> Option<String> {
    let t = raw.trim();
    let t = t.strip_prefix('@').unwrap_or(t);
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.to_lowercase())
}

impl Allowlist {
    pub fn new(raw: &[String]) -> Self {
        let mut entries = BTreeSet::new();
        let mut wildcard = false;
        for e in raw {
            let Some(n) = normalize(e) else { continue };
            if n == "*" {
                wildcard = true;
                continue;
            }
            entries.insert(n);
        }
        Allowlist { entries, wildcard }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && !self.wildcard
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn open_to_everyone(&self) -> bool {
        self.wildcard
    }

    pub fn allows(&self, candidate: &str) -> bool {
        if self.is_empty() {
            return false;
        }
        if self.wildcard {
            return true;
        }
        match normalize(candidate) {
            Some(c) => self.entries.contains(&c),
            None => false,
        }
    }

    pub fn entries(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }

    pub fn allows_any<'a>(&self, candidates: impl IntoIterator<Item = &'a str>) -> bool {
        if self.is_empty() {
            return false;
        }
        if self.wildcard {
            return true;
        }
        candidates.into_iter().any(|c| self.allows(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Allowlist {
        Allowlist::new(&items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn an_empty_list_refuses_everyone() {
        let a = list(&[]);
        assert!(a.is_empty());
        assert!(!a.allows("123"));
        assert!(!a.allows(""));
        assert!(!a.allows_any(["123", "abc"]));
    }

    #[test]
    fn a_list_of_blanks_is_still_empty_and_refuses() {
        let a = list(&["", "   ", "\t", "@"]);
        assert!(a.is_empty(), "blank entries must not create access");
        assert!(!a.allows("anyone"));
    }

    #[test]
    fn exact_entries_are_matched() {
        let a = list(&["1868769425", "paulus"]);
        assert!(a.allows("1868769425"));
        assert!(a.allows("paulus"));
        assert!(!a.allows("1868769426"));
        assert!(!a.allows("paulusx"));
    }

    #[test]
    fn matching_ignores_case_at_sign_and_surrounding_space() {
        let a = list(&["@Paulus"]);
        assert!(a.allows("paulus"));
        assert!(a.allows("@paulus"));
        assert!(a.allows("  @PAULUS  "));
        assert!(a.allows("Paulus"));
    }

    #[test]
    fn a_wildcard_is_an_explicit_opt_in_that_opens_the_channel() {
        let a = list(&["*"]);
        assert!(!a.is_empty());
        assert!(a.open_to_everyone());
        assert!(a.allows("anyone-at-all"));
        assert!(a.allows(""));
    }

    #[test]
    fn a_wildcard_beside_entries_still_opens_the_channel() {
        let a = list(&["paulus", "*"]);
        assert!(a.open_to_everyone());
        assert!(a.allows("stranger"));
    }

    #[test]
    fn duplicates_collapse_and_are_counted_once() {
        let a = list(&["paulus", "@Paulus", " PAULUS "]);
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn any_of_several_candidates_can_match() {
        let a = list(&["paulus"]);
        assert!(a.allows_any(["99999", "paulus"]));
        assert!(!a.allows_any(["99999", "stranger"]));
        assert!(!list(&[]).allows_any(["paulus"]));
    }
}
