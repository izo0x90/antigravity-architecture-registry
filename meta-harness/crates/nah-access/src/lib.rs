//! What each caller may do. Every tool and route names the one capability
//! it needs; a role is a set of capabilities, configured; a caller is the
//! user, home (relaying the user), an agent in a role, or no one known.
//! Nothing asks "is this the lead?": code asks whether a caller has a
//! capability.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::Deserialize;

macro_rules! capabilities {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// One thing a caller may do.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Capability { $($variant),* }

        impl Capability {
            pub const ALL: &'static [Capability] = &[$(Capability::$variant),*];

            /// Its name in config and messages: `view.read`.
            pub fn name(self) -> &'static str {
                match self { $(Capability::$variant => $name),* }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                match name { $($name => Some(Capability::$variant),)* _ => None }
            }
        }
    };
}

capabilities! {
    ViewRead => "view.read",
    ViewDrive => "view.drive",
    PlanEdit => "plan.edit",
    CodeCheck => "code.check",
    SlotsObserve => "slots.observe",
    SlotsManage => "slots.manage",
    SlotsMessage => "slots.message",
    SlotsReport => "slots.report",
    UserNotify => "user.notify",
    RequestsAnswer => "requests.answer",
    PlansApprove => "plans.approve",
    SessionsControl => "sessions.control",
    ChangesStage => "changes.stage",
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A set of capabilities: one bit each.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct CapabilitySet(u16);

const _: () = assert!(Capability::ALL.len() <= u16::BITS as usize, "a capability set holds every capability");

impl CapabilitySet {
    pub fn all() -> Self {
        Capability::ALL.iter().copied().collect()
    }

    pub fn contains(self, c: Capability) -> bool {
        self.0 & (1 << c as u16) != 0
    }

    pub fn insert(&mut self, c: Capability) {
        self.0 |= 1 << c as u16;
    }

    /// Its bits: one per capability, for keying by set.
    pub fn bits(self) -> u16 {
        self.0
    }

    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL.iter().copied().filter(move |c| self.contains(*c))
    }
}

impl FromIterator<Capability> for CapabilitySet {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        let mut set = Self::default();
        for c in iter {
            set.insert(c);
        }
        set
    }
}

/// Who is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    /// The user, in the cockpit.
    User,
    /// Home, relaying what the user does.
    Home,
    /// An agent, in a role.
    Agent { role: Arc<str> },
    /// No one known: no credentials.
    Anonymous,
}

impl Principal {
    pub fn agent(role: &str) -> Self {
        Self::Agent { role: role.into() }
    }
}

/// The roles every configuration must have.
pub const REQUIRED_ROLES: [&str; 4] = ["lead", "sub", "worker", "external"];

/// The roles, and the one check.
#[derive(Clone, Debug)]
pub struct Access {
    roles: HashMap<Arc<str>, CapabilitySet>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    roles: HashMap<String, RoleEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleEntry {
    can: Vec<String>,
}

impl Access {
    /// The shipped roles (`config/access.toml`).
    pub fn defaults() -> Self {
        let mut access = Self { roles: HashMap::new() };
        access.apply(include_str!("../config/access.toml")).expect("the shipped access.toml is valid");
        access
    }

    /// With a project's `.nah/access.toml` over the defaults: a role named
    /// there replaces the default role of that name.
    pub fn with_overrides(mut self, text: &str) -> Result<Self, String> {
        self.apply(text)?;
        Ok(self)
    }

    fn apply(&mut self, text: &str) -> Result<(), String> {
        let file: File = toml::from_str(text).map_err(|e| format!("access: {e}"))?;
        for (name, entry) in file.roles {
            let mut set = CapabilitySet::default();
            for can in &entry.can {
                let c = Capability::from_name(can).ok_or_else(|| {
                    let known: Vec<&str> = Capability::ALL.iter().map(|c| c.name()).collect();
                    format!("access: role {name}: unknown capability '{can}' (known: {})", known.join(", "))
                })?;
                set.insert(c);
            }
            self.roles.insert(name.into(), set);
        }
        let missing: Vec<&str> = REQUIRED_ROLES.iter().copied().filter(|r| !self.roles.contains_key(*r)).collect();
        if !missing.is_empty() {
            return Err(format!("access: missing role(s): {}", missing.join(", ")));
        }
        Ok(())
    }

    /// What a role may do, if there's such a role.
    pub fn role(&self, name: &str) -> Option<CapabilitySet> {
        self.roles.get(name).copied()
    }

    /// What a caller may do: the user and home everything, no one known
    /// nothing, an agent its role's (nothing for a role that doesn't exist).
    pub fn of(&self, who: &Principal) -> CapabilitySet {
        match who {
            Principal::User | Principal::Home => CapabilitySet::all(),
            Principal::Anonymous => CapabilitySet::default(),
            Principal::Agent { role } => self.role(role).unwrap_or_default(),
        }
    }

    pub fn allows(&self, who: &Principal, needs: Capability) -> bool {
        self.of(who).contains(needs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> CapabilitySet {
        names.iter().map(|n| Capability::from_name(n).unwrap()).collect()
    }

    #[test]
    fn every_capability_has_a_name_that_reads_back() {
        for c in Capability::ALL {
            assert_eq!(Capability::from_name(c.name()), Some(*c));
        }
        assert_eq!(Capability::from_name("view.reed"), None);
    }

    #[test]
    fn the_shipped_roles_are_as_agreed() {
        let access = Access::defaults();
        assert_eq!(access.role("lead"), Some(set(&["view.read", "view.drive", "plan.edit", "code.check", "slots.observe", "slots.manage", "slots.message", "user.notify"])));
        assert_eq!(access.role("sub"), Some(set(&["view.read", "slots.report", "user.notify"])));
        assert_eq!(access.role("worker"), Some(set(&["view.read", "plan.edit", "code.check", "user.notify"])));
        assert_eq!(access.role("external"), Some(set(&["view.read"])));
    }

    #[test]
    fn the_user_and_home_can_do_everything_and_no_one_known_nothing() {
        let access = Access::defaults();
        for c in Capability::ALL {
            assert!(access.allows(&Principal::User, *c) && access.allows(&Principal::Home, *c));
            assert!(!access.allows(&Principal::Anonymous, *c));
        }
        assert!(!access.allows(&Principal::agent("lead"), Capability::RequestsAnswer), "only the user answers requests");
        assert!(!access.allows(&Principal::agent("no-such-role"), Capability::ViewRead), "an unknown role can do nothing");
    }

    #[test]
    fn a_project_replaces_whole_roles() {
        let access = Access::defaults().with_overrides("[roles.sub]\ncan = [\"view.read\"]\n").unwrap();
        assert_eq!(access.role("sub"), Some(set(&["view.read"])), "replaced, not merged");
        assert_eq!(access.role("lead"), Access::defaults().role("lead"), "the rest unchanged");
    }

    #[test]
    fn a_mistake_in_the_config_is_refused_with_what_to_fix() {
        let err = Access::defaults().with_overrides("[roles.sub]\ncan = [\"view.reed\"]\n").unwrap_err();
        assert!(err.starts_with("access: role sub: unknown capability 'view.reed' (known: view.read, "), "{err}");
        assert!(Access::defaults().with_overrides("[roles.sub]\ncan = []\nextra = 1\n").is_err(), "unknown keys are refused");
        assert!(Access::defaults().with_overrides("[role.sub]\ncan = []\n").is_err(), "misspelled tables too");
    }
}
