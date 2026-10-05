//! Motore delle metriche di nextm.
//!
//! I moduli `cpu`, `smooth`, `level`, `ram`, `net` e `saturation` contengono solo logica pura,
//! testabile ovunque. Il modulo `sys` legge i contatori di Windows.

pub mod cpu;
pub mod disk;
pub mod inspect;
pub mod level;
pub mod net;
pub mod ram;
pub mod saturation;
pub mod smooth;
pub mod temp;

#[cfg(windows)]
pub mod sys;
