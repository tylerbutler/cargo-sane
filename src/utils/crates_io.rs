//! Crates.io API client

use anyhow::{Context, Result};
use semver::Version;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

const CRATES_IO_API: &str = "https://crates.io/api/v1";
const USER_AGENT: &str = "cargo-sane (https://github.com/yourusername/cargo-sane)";

#[derive(Debug, Deserialize)]
pub struct CrateResponse {
    #[serde(rename = "crate")]
    pub krate: CrateInfo,
}

#[derive(Debug, Deserialize)]
pub struct CrateInfo {
    pub name: String,
    pub newest_version: String,
    pub description: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct VersionsResponse {
    pub versions: Vec<VersionInfo>,
}

#[derive(Debug, Deserialize)]
pub struct VersionInfo {
    pub num: String,
    pub yanked: bool,
}

pub struct CratesIoClient {
    client: reqwest::blocking::Client,
    /// Cache for latest version lookups (crate_name -> version)
    version_cache: RwLock<HashMap<String, Version>>,
}

impl CratesIoClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            version_cache: RwLock::new(HashMap::new()),
        })
    }

    /// Get the latest version of a crate (cached)
    pub fn get_latest_version(&self, crate_name: &str) -> Result<Version> {
        // Check cache first (read lock)
        if let Some(version) = self.version_cache.read().unwrap().get(crate_name) {
            return Ok(version.clone());
        }

        // Cache miss - fetch from API
        let version = self.fetch_latest_version(crate_name)?;

        // Store in cache (write lock)
        self.version_cache
            .write()
            .unwrap()
            .insert(crate_name.to_string(), version.clone());

        Ok(version)
    }

    /// Fetch latest version from crates.io API (uncached)
    fn fetch_latest_version(&self, crate_name: &str) -> Result<Version> {
        let url = format!("{}/crates/{}", CRATES_IO_API, crate_name);

        let response = self
            .client
            .get(&url)
            .send()
            .context(format!("Failed to fetch info for crate: {}", crate_name))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Crates.io API returned error for {}: {}",
                crate_name,
                response.status()
            );
        }

        let crate_response: CrateResponse = response.json().context(format!(
            "Failed to parse response for crate: {}",
            crate_name
        ))?;

        let version = Version::parse(&crate_response.krate.newest_version).context(format!(
            "Failed to parse version {} for crate {}",
            crate_response.krate.newest_version, crate_name
        ))?;

        Ok(version)
    }

    /// Get all versions of a crate (non-yanked only)
    pub fn get_versions(&self, crate_name: &str) -> Result<Vec<Version>> {
        let url = format!("{}/crates/{}/versions", CRATES_IO_API, crate_name);

        let response = self.client.get(&url).send().context(format!(
            "Failed to fetch versions for crate: {}",
            crate_name
        ))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Crates.io API returned error for {}: {}",
                crate_name,
                response.status()
            );
        }

        let versions_response: VersionsResponse = response.json().context(format!(
            "Failed to parse versions for crate: {}",
            crate_name
        ))?;

        let versions: Vec<Version> = versions_response
            .versions
            .iter()
            .filter(|v| !v.yanked)
            .filter_map(|v| Version::parse(&v.num).ok())
            .collect();

        Ok(versions)
    }
}

impl Default for CratesIoClient {
    fn default() -> Self {
        Self::new().expect("Failed to create CratesIoClient")
    }
}
