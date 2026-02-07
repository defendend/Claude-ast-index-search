//! Platform enum and utilities

use std::fmt;
use std::str::FromStr;
use anyhow::{Result, bail};

/// Supported platforms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
    Desktop,
    Aurora,
}

impl FromStr for Platform {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "android" => Ok(Platform::Android),
            "ios" => Ok(Platform::Ios),
            "desktop" => Ok(Platform::Desktop),
            "aurora" => Ok(Platform::Aurora),
            _ => bail!("Unknown platform: {}. Use 'android', 'ios', 'desktop', or 'aurora'", s),
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::Android => write!(f, "android"),
            Platform::Ios => write!(f, "ios"),
            Platform::Desktop => write!(f, "desktop"),
            Platform::Aurora => write!(f, "aurora"),
        }
    }
}

impl Platform {
    pub fn is_android(&self) -> bool {
        matches!(self, Platform::Android)
    }

    pub fn is_ios(&self) -> bool {
        matches!(self, Platform::Ios)
    }

    pub fn is_desktop(&self) -> bool {
        matches!(self, Platform::Desktop)
    }

    pub fn is_aurora(&self) -> bool {
        matches!(self, Platform::Aurora)
    }
}
