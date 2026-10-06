const CATALOGUE: &[(&str, &str)] = &[
    (
        "invalid security control list",
        "ungültige Sicherheitssteuerungsliste",
    ),
    (
        "kernel capability range is unsupported",
        "Kernel-Capability-Bereich wird nicht unterstützt",
    ),
    (
        "requested capability is unavailable",
        "angeforderte Capability ist nicht verfügbar",
    ),
    (
        "seccomp native architecture is unsupported",
        "native Architektur wird für seccomp nicht unterstützt",
    ),
    (
        "capability reduction and seccomp require no_new_privs",
        "Capability-Reduktion und seccomp benötigen no_new_privs",
    ),
    (
        "no-new-privs requires yes or inherit",
        "no-new-privs benötigt yes oder inherit",
    ),
    (
        "invalid applied security controls",
        "ungültige angewendete Sicherheitssteuerung",
    ),
    (
        "requested security control is unavailable",
        "angeforderte Sicherheitssteuerung ist nicht verfügbar",
    ),
    (
        "duplicate security control",
        "doppelte Sicherheitssteuerungsoption",
    ),
    (
        "Optional security: --no-new-privs yes|inherit; --cap-drop NAME,...|all|inherit; --seccomp-deny NAME,...|inherit",
        "Optionale Sicherheit: --no-new-privs yes|inherit; --cap-drop NAME,...|all|inherit; --seccomp-deny NAME,...|inherit",
    ),
    (
        "Capability reduction and seccomp enable no_new_privs. Queue and Group defaults use the --job- prefix. Inspect supported names with host --json. A deny list is not a sandbox.",
        "Capability-Reduktion und seccomp aktivieren no_new_privs. Standardwerte für Queues und Gruppen verwenden das Präfix --job-. Unterstützte Namen findest du mit host --json. Eine Verbotsliste ist keine Sandbox.",
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
    ["Optional security: --no-new-privs yes|inherit; --cap-drop NAME,...|all|inherit; --seccomp-deny NAME,...|inherit", "Capability reduction and seccomp enable no_new_privs. Queue and Group defaults use the --job- prefix. Inspect supported names with host --json. A deny list is not a sandbox."].map(message).join("\n")
}
