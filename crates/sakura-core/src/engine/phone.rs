//! The setup of the phone page, as `sakura cockpit phone --json` prints it: whether Tailscale is there and signed
//! in, this Mac's name on the overlay network, the link, which devices may open it and which devices the login has. Every
//! field is optional: an older engine prints none of it.

use serde::Deserialize;

/// Where the setup stands, in the engine's words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PhoneSetup {
    /// `not_installed`, `logged_out`, `no_device` or `ready`.
    #[serde(default)]
    pub state: String,
    /// What is so and what to do, in plain words.
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub port: Option<u16>,
    /// The full link as the engine knows it (its MagicDNS name and port).
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub devices: Vec<PhoneDevice>,
}

/// One device of the overlay network of this login.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PhoneDevice {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub online: bool,
    /// Whether it may open the phone page.
    #[serde(default)]
    pub allowed: bool,
    #[serde(default)]
    pub is_this_mac: bool,
}

impl PhoneSetup {
    /// Reads what the engine printed.
    pub fn parse(text: &[u8]) -> Result<PhoneSetup, serde_json::Error> {
        serde_json::from_slice(text)
    }

    /// The devices that may open the page.
    pub fn allowed(&self) -> impl Iterator<Item = &PhoneDevice> {
        self.devices.iter().filter(|device| device.allowed)
    }

    /// The devices that may not yet, which the person can allow.
    pub fn allowable(&self) -> impl Iterator<Item = &PhoneDevice> {
        self.devices.iter().filter(|device| !device.allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/engine-phone.json");

    #[test]
    fn the_setup_names_the_state_the_link_and_the_devices_by_name() {
        let setup = PhoneSetup::parse(FIXTURE.as_bytes()).expect("the fixture");
        assert_eq!(setup.state, "ready");
        assert_eq!(
            setup.link.as_deref(),
            Some("http://mac.example.ts.net:8787/")
        );
        let allowed: Vec<_> = setup.allowed().map(|device| device.name.as_str()).collect();
        assert_eq!(allowed, ["Phone"]);
        let others: Vec<_> = setup
            .allowable()
            .map(|device| device.name.as_str())
            .collect();
        assert_eq!(others, ["Mac", "Tablet"]);
    }

    #[test]
    fn an_engine_that_says_nothing_of_the_setup_gives_an_empty_one() {
        let setup = PhoneSetup::parse(b"{}").expect("empty");
        assert_eq!(setup.state, "");
        assert!(setup.devices.is_empty());
    }
}
