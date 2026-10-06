const CATALOGUE: &[(&str, &str)] = &[
    (
        "state schema requires an explicit offline migration",
        "diese Zustandsversion benötigt eine explizite Offline-Migration",
    ),
    (
        "network link file and record names differ",
        "Datei und Datensatz einer Netzwerkverbindung haben unterschiedliche Namen",
    ),
    (
        "finish or cancel Jobs and stop network helpers under the previous service before migration",
        "beende oder storniere die Jobs und stoppe Netzwerk-Hilfsprozesse über den bisherigen Dienst vor der Migration",
    ),
    (
        "network helpers of this state are still running; stop them before a backup",
        "Netzwerk-Hilfsprozesse dieses Zustands laufen noch; stoppe sie vor einer Sicherung",
    ),
    (
        "Jobs recorded as starting, running, suspended or stopping: {ids}; let the service finish or cancel them, or pass --interrupted to copy these records as they are",
        "Jobs, die als startend, laufend, angehalten oder endend eingetragen sind: {ids}; lass den Dienst sie beenden oder storniere sie, oder gib --interrupted an, um diese Einträge unverändert zu kopieren",
    ),
    (
        "the supervisors of these Jobs are still running: {ids}; --interrupted copies only records whose processes have ended",
        "die Supervisoren dieser Jobs laufen noch: {ids}; --interrupted kopiert nur Einträge, deren Prozesse beendet sind",
    ),
    ("--backup is required", "gib --backup an"),
    ("--destination is required", "gib --destination an"),
    (
        "--profile must be ordinary or legacy",
        "wähle für --profile ordinary oder legacy",
    ),
    ("--source is required", "gib --source an"),
    (
        "Job directory and record identities differ",
        "Job-Verzeichnis und Datensatz haben unterschiedliche Kennungen",
    ),
    (
        "Job references a missing Queue identity",
        "Job verweist auf eine fehlende Queue-Kennung",
    ),
    (
        "backup changed while being copied",
        "die Sicherung wurde während des Kopierens verändert",
    ),
    (
        "backup data must be a directory, not a symlink",
        "Sicherungsdaten müssen in einem Verzeichnis liegen, nicht in einem symbolischen Link",
    ),
    (
        "backup entry is not a regular file",
        "Sicherungseintrag ist keine reguläre Datei",
    ),
    (
        "backup integrity check failed",
        "Integritätsprüfung der Sicherung fehlgeschlagen",
    ),
    ("destination already exists", "das Ziel existiert bereits"),
    (
        "destination requires a directory name",
        "gib einen Verzeichnisnamen als Ziel an",
    ),
    (
        "finish or cancel all active and queued Jobs before migration or backup",
        "beende oder storniere alle aktiven und wartenden Jobs vor Migration oder Sicherung",
    ),
    (
        "invalid Job directory name",
        "ungültiger Name eines Job-Verzeichnisses",
    ),
    (
        "invalid relative backup path",
        "ungültiger relativer Sicherungspfad",
    ),
    (
        "invalid state command or options",
        "ungültiger Zustandsbefehl oder ungültige Optionen",
    ),
    (
        "legacy exec state exists; migrate explicitly or select JOB_STATE_DIR",
        "alter exec-Zustand existiert; migriere ihn explizit oder wähle JOB_STATE_DIR",
    ),
    (
        "migration requires an explicit --profile ordinary or --profile legacy",
        "wähle zur Migration explizit --profile ordinary oder --profile legacy",
    ),
    (
        "next Job identity does not exceed all persisted identities",
        "die nächste Job-Kennung liegt nicht über allen gespeicherten Kennungen",
    ),
    (
        "restore accepts --source and --destination only",
        "restore akzeptiert nur --source und --destination",
    ),
    (
        "source changed during backup",
        "die Quelle wurde während der Sicherung verändert",
    ),
    (
        "source, backup and destination must be separate directories",
        "Quelle, Sicherung und Ziel müssen getrennte Verzeichnisse sein",
    ),
    (
        "state contains a symlink or unsupported special file",
        "der Zustand enthält einen symbolischen Link oder eine nicht unterstützte Spezialdatei",
    ),
    (
        "state must be owned by the current service user",
        "der Zustand muss dem aktuellen Dienstbenutzer gehören",
    ),
    (
        "state paths must be UTF-8",
        "Zustandspfade müssen UTF-8 verwenden",
    ),
    (
        "unexpected entry in the Jobs directory",
        "unerwarteter Eintrag im Job-Verzeichnis",
    ),
    (
        "unknown state command option",
        "unbekannte Option für den Zustandsbefehl",
    ),
    (
        "unsupported backup manifest version",
        "nicht unterstützte Version des Sicherungsmanifests",
    ),
    (
        "unsupported state schema or resource vocabulary",
        "nicht unterstützte Zustandsversion oder Ressourcenbezeichnungen",
    ),
    (
        "unversioned state requires an explicit offline migration",
        "Zustand ohne Versionsangabe benötigt eine explizite Offline-Migration",
    ),
    (
        "usage: job state validate|backup|migrate|restore --source PATH [--destination PATH] [--backup PATH] [--dry-run] [--interrupted]",
        "Aufruf: job state validate|backup|migrate|restore --source PFAD [--destination PFAD] [--backup PFAD] [--dry-run]",
    ),
    (
        "versioned Job is missing its Queue identity",
        "versionierter Job hat keine Queue-Kennung",
    ),
];

pub fn message(english: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
            .to_owned()
    } else {
        english.to_owned()
    }
}
