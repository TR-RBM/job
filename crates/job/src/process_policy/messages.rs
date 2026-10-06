const CATALOGUE: &[(&str, &str)] = &[
    (
        "invalid applied process controls",
        "ungültige angewendete Prozesssteuerung",
    ),
    (
        "invalid CPU or NUMA node list",
        "ungültige CPU- oder NUMA-Knotenliste",
    ),
    ("invalid NUMA policy", "ungültige NUMA-Richtlinie"),
    (
        "rlimit value is outside the kernel range",
        "rlimit-Wert liegt außerhalb des Kernel-Wertebereichs",
    ),
    (
        "rlimit soft value exceeds hard value",
        "weicher rlimit-Wert überschreitet den harten Wert",
    ),
    (
        "rlimit scheduling value is outside its range",
        "rlimit-Schedulingwert liegt außerhalb seines Wertebereichs",
    ),
    (
        "rlimit requires NAME=SOFT:HARD",
        "rlimit benötigt NAME=WEICH:HART",
    ),
    (
        "unsupported rlimit resource",
        "nicht unterstützte rlimit-Ressource",
    ),
    (
        "rlimit requires a nonnegative integer in native units",
        "rlimit benötigt eine nichtnegative Ganzzahl in nativen Einheiten",
    ),
    (
        "requested process control is unavailable",
        "angeforderte Prozesssteuerung ist nicht verfügbar",
    ),
    (
        "CPU affinity includes unavailable CPUs",
        "CPU-Affinität enthält nicht verfügbare CPUs",
    ),
    (
        "NUMA policy includes unavailable memory nodes",
        "NUMA-Richtlinie enthält nicht verfügbare Speicherknoten",
    ),
    (
        "duplicate process control",
        "doppelte Prozesssteuerungsoption",
    ),
    (
        "Optional launch controls: --cpu-affinity LIST|inherit; --numa-policy inherit|default|local|bind:LIST|interleave:LIST|preferred:N; --rlimit NAME=SOFT:HARD",
        "Optionale Startsteuerung: --cpu-affinity LIST|inherit; --numa-policy inherit|default|local|bind:LIST|interleave:LIST|preferred:N; --rlimit NAME=WEICH:HART",
    ),
    (
        "Collection defaults use --job-cpu-affinity, --job-numa-policy and --job-rlimit. rlimits inherit independently by resource. Placement does not impose cgroup containment.",
        "Sammlungsdefaults verwenden --job-cpu-affinity, --job-numa-policy und --job-rlimit. rlimits werden je Ressource vererbt. Platzierung erzwingt keine cgroup-Abgrenzung.",
    ),
    (
        "Process launch controls could not be applied",
        "Prozessstartsteuerung konnte nicht angewendet werden",
    ),
];
pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(en, _)| *en == key)
            .map_or(key, |(_, de)| *de)
            .to_owned()
    } else {
        key.to_owned()
    }
}
pub fn help() -> String {
    ["Optional launch controls: --cpu-affinity LIST|inherit; --numa-policy inherit|default|local|bind:LIST|interleave:LIST|preferred:N; --rlimit NAME=SOFT:HARD", "Collection defaults use --job-cpu-affinity, --job-numa-policy and --job-rlimit. rlimits inherit independently by resource. Placement does not impose cgroup containment."].map(message).join("\n")
}
