//! Dependency representation

use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub current_version: Version,
    pub latest_version: Option<Version>,
    pub is_direct: bool,
    /// Whether this dependency inherits from workspace.dependencies
    pub is_workspace_inherited: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateType {
    Patch,
    Minor,
    Major,
    UpToDate,
}

impl Dependency {
    pub fn new(name: String, current_version: Version, is_direct: bool) -> Self {
        Self {
            name,
            current_version,
            latest_version: None,
            is_direct,
            is_workspace_inherited: false,
        }
    }

    pub fn with_latest(mut self, latest: Version) -> Self {
        self.latest_version = Some(latest);
        self
    }

    pub fn with_workspace_inherited(mut self, inherited: bool) -> Self {
        self.is_workspace_inherited = inherited;
        self
    }

    /// Determine the type of update available
    pub fn update_type(&self) -> UpdateType {
        match &self.latest_version {
            None => UpdateType::UpToDate,
            Some(latest) => {
                if latest <= &self.current_version {
                    UpdateType::UpToDate
                } else if latest.major > self.current_version.major {
                    UpdateType::Major
                } else if latest.minor > self.current_version.minor {
                    UpdateType::Minor
                } else {
                    UpdateType::Patch
                }
            }
        }
    }

    /// Check if update is available
    pub fn has_update(&self) -> bool {
        self.update_type() != UpdateType::UpToDate
    }
}
