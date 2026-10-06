use std::{collections::BTreeMap, fmt, str::FromStr};

use thiserror::Error;

use crate::{domain::TopicPath, schema::SchemaIssue};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LinkName(String);

impl LinkName {
    pub fn parse(input: &str) -> Result<Self, LinkNameParseError> {
        if input.is_empty() {
            return Err(LinkNameParseError::Empty);
        }
        if input.chars().any(char::is_control) {
            return Err(LinkNameParseError::ControlCharacter);
        }
        if let Some(character) = input
            .chars()
            .find(|character| matches!(character, '/' | '*' | '?' | '[' | ']' | '{' | '}' | '\\'))
        {
            return Err(LinkNameParseError::ReservedCharacter(character));
        }
        Ok(Self(input.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LinkName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for LinkName {
    type Err = LinkNameParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum LinkNameParseError {
    #[error("a link name may not be empty")]
    Empty,
    #[error("a link name may not contain control characters")]
    ControlCharacter,
    #[error("a link name contains reserved character `{0}`")]
    ReservedCharacter(char),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkDefinition {
    name: LinkName,
    mount: TopicPath,
    target: TopicPath,
}

impl LinkDefinition {
    pub fn new(
        name: LinkName,
        mount: TopicPath,
        target: TopicPath,
    ) -> Result<Self, LinkBuildError> {
        if mount.is_system() {
            return Err(LinkBuildError::SystemMount { mount });
        }
        if target.is_system() {
            return Err(LinkBuildError::SystemTarget { target });
        }
        if mount.is_prefix_of(&target) || target.is_prefix_of(&mount) {
            return Err(LinkBuildError::OverlappingSourceAndMount { mount, target });
        }
        Ok(Self {
            name,
            mount,
            target,
        })
    }

    #[must_use]
    pub fn name(&self) -> &LinkName {
        &self.name
    }

    #[must_use]
    pub fn mount(&self) -> &TopicPath {
        &self.mount
    }

    #[must_use]
    pub fn target(&self) -> &TopicPath {
        &self.target
    }

    pub(crate) fn to_alias(&self, canonical: &TopicPath) -> Option<TopicPath> {
        canonical.rebase(&self.target, &self.mount)
    }

    pub(crate) fn to_canonical(&self, alias: &TopicPath) -> Option<TopicPath> {
        alias.rebase(&self.mount, &self.target)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum LinkBuildError {
    #[error("link mount `{mount}` is in the reserved system namespace")]
    SystemMount { mount: TopicPath },
    #[error("link target `{target}` is in the reserved system namespace")]
    SystemTarget { target: TopicPath },
    #[error("link mount `{mount}` overlaps its target subtree `{target}`")]
    OverlappingSourceAndMount { mount: TopicPath, target: TopicPath },
}

#[derive(Clone, Debug)]
pub(crate) struct InstalledLink {
    definition: LinkDefinition,
    denial: Option<SchemaIssue>,
}

impl InstalledLink {
    pub(crate) fn new(definition: LinkDefinition, denial: Option<SchemaIssue>) -> Self {
        Self { definition, denial }
    }

    pub(crate) fn definition(&self) -> &LinkDefinition {
        &self.definition
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.denial.is_none()
    }

    pub(crate) fn denial(&self) -> Option<&SchemaIssue> {
        self.denial.as_ref()
    }

    pub(crate) fn set_denial(&mut self, denial: Option<SchemaIssue>) {
        self.denial = denial;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LinkRegistry {
    links: BTreeMap<LinkName, InstalledLink>,
}

impl LinkRegistry {
    pub(crate) fn len(&self) -> usize {
        self.links.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &InstalledLink> {
        self.links.values()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut InstalledLink> {
        self.links.values_mut()
    }

    pub(crate) fn get(&self, name: &LinkName) -> Option<&InstalledLink> {
        self.links.get(name)
    }

    pub(crate) fn remove(&mut self, name: &LinkName) -> Option<InstalledLink> {
        self.links.remove(name)
    }

    pub(crate) fn install(
        &mut self,
        definition: LinkDefinition,
        denial: Option<SchemaIssue>,
    ) -> Result<Option<InstalledLink>, LinkInstallError> {
        for existing in self.links.values() {
            if existing.definition.name == definition.name {
                continue;
            }
            let existing = &existing.definition;
            if existing.mount.is_prefix_of(&definition.mount)
                || definition.mount.is_prefix_of(&existing.mount)
            {
                return Err(LinkInstallError::OverlappingMounts {
                    candidate: definition.mount.clone(),
                    existing: existing.mount.clone(),
                });
            }
            if existing.mount.is_prefix_of(&definition.target) {
                return Err(LinkInstallError::TargetThroughLink {
                    target: definition.target.clone(),
                    mount: existing.mount.clone(),
                });
            }
            if definition.mount.is_prefix_of(&existing.target) {
                return Err(LinkInstallError::TargetThroughLink {
                    target: existing.target.clone(),
                    mount: definition.mount.clone(),
                });
            }
        }
        Ok(self.links.insert(
            definition.name.clone(),
            InstalledLink::new(definition, denial),
        ))
    }

    pub(crate) fn resolve_alias(&self, topic: &TopicPath) -> Option<(&InstalledLink, TopicPath)> {
        self.links.values().find_map(|link| {
            link.definition
                .to_canonical(topic)
                .map(|canonical| (link, canonical))
        })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum LinkInstallError {
    #[error("link mount `{mount}` collides with existing node `{topic}`")]
    DestinationCollision { mount: TopicPath, topic: TopicPath },
    #[error("link mounts `{candidate}` and `{existing}` overlap")]
    OverlappingMounts {
        candidate: TopicPath,
        existing: TopicPath,
    },
    #[error("link target `{target}` is reached through link mount `{mount}`")]
    TargetThroughLink { target: TopicPath, mount: TopicPath },
}
