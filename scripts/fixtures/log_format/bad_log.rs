//! The bad fixture: records that do not name a subsystem.
use log::info;

pub fn without_a_prefix() {
    info!("the storage is open");
}

pub fn with_a_wrong_subsystem() {
    log::warn!("storage: the entry is missing");
}

pub fn with_a_message_in_a_variable() {
    let message = "the storage is open";
    log::debug!("{message}");
}

pub fn with_a_raw_literal() {
    log::error!(r#"the raw literal has no subsystem"#);
}
