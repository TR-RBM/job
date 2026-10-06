pub mod access;
pub mod cli;
pub mod depth;
pub mod health;
pub mod journal;
pub mod limits;
mod messages;
pub mod mode;
pub mod output;
pub use messages::message;

pub fn opened(
    store: &crate::store::Store,
    config: &crate::config::Config,
    graph: &crate::objects::Graph,
    ledger: &crate::pressure::control::Ledger,
) -> std::io::Result<()> {
    health::started();
    health::cgroup_required(config.cgroup.required);
    journal::open(store, &config.events, graph, ledger)
}

pub fn render_host(info: &crate::model::HostInfo) -> Vec<String> {
    info.health
        .iter()
        .flat_map(health::render)
        .chain(info.surrounding_limits.iter().flat_map(limits::render))
        .collect()
}
