//! The good fixture: every record names a subsystem.
use log::info;

pub fn compliant() {
    info!("engine.storage: the storage is open");
    log::warn!("engine.storage: the entry is missing");
    log::debug!("engine.storage: {} entries", 3);
    log::error!("engine.storage: the archive is unreadable");
    log::trace!("engine.storage: the entry is read");
}

pub fn a_subsystem_that_is_known_but_wrong_for_the_module() {
    log::info!("platform.render: the surface is created");
}
