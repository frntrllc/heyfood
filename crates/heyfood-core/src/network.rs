//! Validated service endpoints and transport policy.

use std::fmt;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

/// Transport policy applied at every service URL ingress.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkPolicy {
    /// Permit plain HTTP only when the parsed host is an exact loopback host.
    pub allow_plaintext_loopback: bool,
}

impl NetworkPolicy {
    pub const HTTPS_ONLY: Self = Self {
        allow_plaintext_loopback: false,
    };

    pub const DEVELOPMENT: Self = Self {
        allow_plaintext_loopback: true,
    };
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self::HTTPS_ONLY
    }
}

/// A service base URL that has passed the heyfood network policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServiceUrl(Url);

impl ServiceUrl {
    /// Parse and validate a service URL.
    ///
    /// Userinfo, query strings, and fragments are rejected so credentials and
    /// endpoint-specific data cannot accidentally become part of a base URL.
    pub fn parse(value: &str, policy: NetworkPolicy) -> Result<Self, ServiceUrlError> {
        let candidate = value.trim();
        if candidate.is_empty() {
            return Err(ServiceUrlError::Empty);
        }

        let url = Url::parse(candidate).map_err(|_| ServiceUrlError::Invalid)?;
        validate_common(&url, policy)?;
        if url.query().is_some() {
            return Err(ServiceUrlError::Query);
        }
        if url.fragment().is_some() {
            return Err(ServiceUrlError::Fragment);
        }

        Ok(Self(url))
    }

    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.0
    }

    #[must_use]
    pub fn is_plaintext_loopback(&self) -> bool {
        self.0.scheme() == "http" && self.0.host().is_some_and(is_exact_loopback)
    }
}

/// A validated browser destination. Unlike a service base URL, an OAuth
/// authorization destination may contain a query string.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BrowserUrl(Url);

impl BrowserUrl {
    pub fn parse(value: &str, policy: NetworkPolicy) -> Result<Self, ServiceUrlError> {
        let candidate = value.trim();
        if candidate.is_empty() {
            return Err(ServiceUrlError::Empty);
        }

        let url = Url::parse(candidate).map_err(|_| ServiceUrlError::Invalid)?;
        validate_common(&url, policy)?;
        if url.fragment().is_some() {
            return Err(ServiceUrlError::Fragment);
        }
        Ok(Self(url))
    }

    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

/// An HTTP(S) forward proxy URL. Plain HTTP proxies are standard and may carry
/// encrypted HTTPS tunnels, so this is intentionally distinct from a service
/// endpoint. Embedded credentials, query, and fragments are forbidden.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProxyUrl(Url);

impl ProxyUrl {
    pub fn parse(value: &str) -> Result<Self, ServiceUrlError> {
        let candidate = value.trim();
        if candidate.is_empty() {
            return Err(ServiceUrlError::Empty);
        }
        let url = Url::parse(candidate).map_err(|_| ServiceUrlError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ServiceUrlError::UnsupportedScheme);
        }
        if url.host().is_none() {
            return Err(ServiceUrlError::MissingHost);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ServiceUrlError::EmbeddedCredentials);
        }
        if url.query().is_some() {
            return Err(ServiceUrlError::Query);
        }
        if url.fragment().is_some() {
            return Err(ServiceUrlError::Fragment);
        }
        Ok(Self(url))
    }

    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Display for BrowserUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn validate_common(url: &Url, policy: NetworkPolicy) -> Result<(), ServiceUrlError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ServiceUrlError::UnsupportedScheme);
    }
    if url.host().is_none() {
        return Err(ServiceUrlError::MissingHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ServiceUrlError::EmbeddedCredentials);
    }
    if url.scheme() == "http"
        && !(policy.allow_plaintext_loopback && url.host().is_some_and(is_exact_loopback))
    {
        return Err(ServiceUrlError::InsecureTransport);
    }
    Ok(())
}

impl fmt::Display for ServiceUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn is_exact_loopback(host: Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address == std::net::Ipv4Addr::LOCALHOST,
        Host::Ipv6(address) => address == std::net::Ipv6Addr::LOCALHOST,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceUrlError {
    Empty,
    Invalid,
    UnsupportedScheme,
    MissingHost,
    EmbeddedCredentials,
    Query,
    Fragment,
    InsecureTransport,
}

impl fmt::Display for ServiceUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "service URL must not be empty",
            Self::Invalid => "service URL is invalid",
            Self::UnsupportedScheme => "service URL must use HTTP or HTTPS",
            Self::MissingHost => "service URL must include a host",
            Self::EmbeddedCredentials => "service URL must not embed credentials",
            Self::Query => "service base URL must not contain a query string",
            Self::Fragment => "service base URL must not contain a fragment",
            Self::InsecureTransport => {
                "plain HTTP is allowed only for an exact loopback development host"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ServiceUrlError {}
