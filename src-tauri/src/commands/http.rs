//! The one HTTP client this app builds, and the identity it sends.
//!
//! Two commands reach the network — the radioid.net user dump and the
//! BrandMeister talkgroup list — and both are somebody else's public service
//! being asked for a bulk file. radioid.net's API terms ask that "automated
//! clients should send a clear `User-Agent` with an app name and contact
//! address", so the string lives here rather than being written out twice and
//! drifting.
//!
//! The address is the repository rather than an email, deliberately: this
//! string is sent by EVERY user of the app on every download, so an email here
//! would put one person's address in every user's request and in the service's
//! logs. The URL 404s until the repo is public (#92) — harmless, since the app
//! name sits right beside it, and it becomes correct the moment that clears.
//!
//! The point of the contact is operational, not credit: it is how radioid.net
//! reaches whoever is responsible for the traffic before deciding to block it.
//!
//! Every request is bounded in time and, at the call site, in bytes. Neither
//! service is polled: both fetches happen only when the operator asks.

use std::time::Duration;

/// Sent on every outbound request.
pub(crate) const USER_AGENT: &str = concat!(
    "WW8LCodeplugMagic/",
    env!("CARGO_PKG_VERSION"),
    " (amateur-radio codeplug editor; +https://github.com/ww8l/codeplug-magic)"
);

/// A client that identifies this app and refuses to hang.
pub(crate) fn client(connect: Duration, total: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect)
        .timeout(total)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app name and version are what a service operator sees in their logs
    /// when they wonder who is pulling their file.
    #[test]
    fn the_user_agent_names_the_app_and_its_version() {
        assert!(USER_AGENT.starts_with("WW8LCodeplugMagic/"), "{USER_AGENT}");
        assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")), "{USER_AGENT}");
        // No bare default: reqwest's own UA says nothing about this app.
        assert!(!USER_AGENT.contains("reqwest"), "{USER_AGENT}");
    }

    /// radioid.net's API terms ask automated clients for "a clear User-Agent
    /// with an app name and contact address". The address is the repository, so
    /// that one person's email is not sent by every user of the app.
    #[test]
    fn the_user_agent_carries_a_contact_address() {
        assert!(
            USER_AGENT.contains("+https://github.com/ww8l/codeplug-magic"),
            "{USER_AGENT}"
        );
        assert!(!USER_AGENT.contains('@'), "no email address: {USER_AGENT}");
    }

    #[test]
    fn a_client_can_be_built_with_bounds() {
        assert!(client(Duration::from_secs(5), Duration::from_secs(30)).is_ok());
    }
}
