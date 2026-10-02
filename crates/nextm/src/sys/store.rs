//! Dove si salvano le impostazioni: registro (normale) o file accanto all'exe (portable).

use std::fs::OpenOptions;
use std::path::PathBuf;

use crate::ini;
use crate::settings::Settings;
use crate::sys::registry;

const KEY: &[u16] = wide!("Software\\nextm");
/// Nome del file che attiva la modalità portable.
pub const PORTABLE_FILE: &str = "nextm.ini";

pub enum Store {
    /// `HKCU\Software\nextm`.
    Registry,
    /// File `nextm.ini` accanto all'eseguibile.
    Portable(PathBuf),
}

/// Esito del rilevamento della modalità.
pub struct Detected {
    pub store: Store,
    /// C'era `nextm.ini` ma la cartella non è scrivibile: si usa il registro e lo si dice.
    pub portable_readonly: bool,
}

impl Store {
    /// Portable se accanto all'exe c'è `nextm.ini` scrivibile; altrimenti registro.
    pub fn detect() -> Detected {
        let path = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(PORTABLE_FILE)));
        match path {
            Some(p) if p.is_file() => {
                if OpenOptions::new().append(true).open(&p).is_ok() {
                    Detected { store: Store::Portable(p), portable_readonly: false }
                } else {
                    Detected { store: Store::Registry, portable_readonly: true }
                }
            }
            _ => Detected { store: Store::Registry, portable_readonly: false },
        }
    }

    pub fn is_portable(&self) -> bool {
        matches!(self, Store::Portable(_))
    }

    /// Carica le impostazioni; ciò che manca prende il default.
    pub fn load(&self) -> Settings {
        match self {
            Store::Registry => Settings::from_lookup(|k| {
                let name: Vec<u16> = k.encode_utf16().chain(core::iter::once(0)).collect();
                registry::get_dword(KEY, &name)
            }),
            Store::Portable(p) => {
                let text = std::fs::read_to_string(p).unwrap_or_default();
                Settings::from_lookup(|k| ini::get_u32(&text, k))
            }
        }
    }

    /// Salva tutte le impostazioni. Il file portable si riscrive in modo atomico
    /// (file temporaneo e poi rinomina).
    pub fn save(&self, s: &Settings) -> bool {
        match self {
            Store::Registry => s.to_pairs().iter().all(|(k, v)| {
                let name: Vec<u16> = k.encode_utf16().chain(core::iter::once(0)).collect();
                registry::set_dword(KEY, &name, *v)
            }),
            Store::Portable(p) => {
                let tmp = p.with_extension("ini.tmp");
                std::fs::write(&tmp, ini::format(&s.to_pairs())).is_ok() && std::fs::rename(&tmp, p).is_ok()
            }
        }
    }
}
