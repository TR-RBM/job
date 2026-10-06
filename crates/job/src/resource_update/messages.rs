const CATALOGUE: &[(&str, &str)] = &[
    (
        "abandon requires an empty resource domain",
        "zum Verwerfen muss der Ressourcenbereich leer sein",
    ),
    (
        "resource update abandoned; completed writes were not rolled back",
        "Ressourcenänderung verworfen; ausgeführte Schreibzugriffe wurden nicht zurückgenommen",
    ),
    (
        "usage: job resource-update OPERATION [--abandon] [--json]",
        "Aufruf: job resource-update VORGANG [--abandon] [--json]",
    ),
    (
        "Preview with --dry-run; inspect progress with resource-update. Supported controls:",
        "Vorschau mit --dry-run; Fortschritt mit resource-update. Unterstützte Regeln:",
    ),
    (
        "Collection updates require an existing domain; requests and launch specifications stay unchanged.",
        "Queue-/Group-Änderungen erfordern einen vorhandenen Bereich; Reservierungen und Startvorgaben bleiben unverändert.",
    ),
    (
        "Exit 75 means pending; query the operation ID instead of submitting again.",
        "Exit 75 bedeutet ausstehend; frage die Vorgangs-ID ab, statt erneut zu starten.",
    ),
    (
        "invalid resource update journal",
        "ungültiges Journal für Ressourcenänderungen",
    ),
    (
        "resource update is pending",
        "Ressourcenänderung ist noch nicht abgeschlossen",
    ),
    (
        "resource update target changed",
        "Ziel der Ressourcenänderung hat sich geändert",
    ),
    (
        "resource control differs from the recorded update",
        "Ressourcenregel weicht von der gespeicherten Änderung ab",
    ),
    (
        "resource update target ended",
        "Ziel der Ressourcenänderung ist nicht mehr vorhanden",
    ),
    (
        "live updates require a local running or suspended Job",
        "Live-Änderungen erfordern einen lokalen laufenden oder angehaltenen Job",
    ),
    (
        "live updates require an existing resource domain",
        "Live-Änderungen erfordern einen vorhandenen Ressourcenbereich",
    ),
    (
        "live updates accept only explicit kernel resource controls",
        "Live-Änderungen erlauben nur explizite Kernel-Ressourcenregeln",
    ),
    (
        "lowering memory.max requires --allow-oom; processes may be killed",
        "das Absenken von memory.max erfordert --allow-oom; Prozesse können beendet werden",
    ),
    ("no such resource update", "keine solche Ressourcenänderung"),
    ("resource update", "Ressourcenänderung"),
    ("applied", "angewendet"),
    ("pending", "ausstehend"),
    ("ended", "Ziel beendet"),
    ("preview", "Vorschau"),
    (
        "usage: job update ID [CONTROLS] [--allow-oom] [--dry-run] [--json]",
        "Aufruf: job update ID [REGELN] [--allow-oom] [--dry-run] [--json]",
    ),
    (
        "usage: job queue|group update PATH [CONTROLS] [--allow-oom] [--dry-run] [--json]",
        "Aufruf: job queue|group update PFAD [REGELN] [--allow-oom] [--dry-run] [--json]",
    ),
    (
        "usage: job resource-update OPERATION [--json]",
        "Aufruf: job resource-update VORGANG [--json]",
    ),
];

pub fn help() -> String {
    [
        message("usage: job update ID [CONTROLS] [--allow-oom] [--dry-run] [--json]"),
        message("usage: job queue|group update PATH [CONTROLS] [--allow-oom] [--dry-run] [--json]"),
        message("usage: job resource-update OPERATION [--abandon] [--json]"),
        message("Preview with --dry-run; inspect progress with resource-update. Supported controls:"),
        "--cpu-limit --cpu-weight --memory-high --memory-max --memory-swap-max\n--io-max --io-weight --io-bfq-weight".to_owned(),
        message("Collection updates require an existing domain; requests and launch specifications stay unchanged."),
        message("lowering memory.max requires --allow-oom; processes may be killed"),
        message("Exit 75 means pending; query the operation ID instead of submitting again."),
    ].join("\n")
}

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de")
        && let Some((_, translated)) = CATALOGUE.iter().find(|(english, _)| *english == key)
    {
        return (*translated).to_owned();
    }
    key.to_owned()
}
