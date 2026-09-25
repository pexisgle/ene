fn main() {
    if std::env::var_os("ENE_ACTION_STAGING_HELPER").is_some() {
        ene_core::run_workspace_staging_helper();
    } else {
        ene_action::run_workspace_effect_worker();
    }
}
