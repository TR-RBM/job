const CATALOGUE: &[(&str, &str)] = &[
    (
        "Collection defaults: --job-execution-profile NAME@REV|none; --job-class NAME@REV|none",
        "Sammlungsvorgaben: --job-execution-profile NAME@REV|none; --job-class NAME@REV|none",
    ),
    (
        "Classes set admission priority only. Direct Job fields override profile defaults; ancestor constraints still apply.",
        "Klassen setzen nur die Startpriorität. Direkte Job-Felder überschreiben Profilvorgaben; übergeordnete Grenzen gelten weiterhin.",
    ),
    (
        "profile composition exceeds the expansion budget",
        "Profilauflösung überschreitet das Schrittbudget",
    ),
    (
        "use a pinned name@revision or none",
        "verwende einen festen Namen@Revision oder none",
    ),
    (
        "unsupported profile value",
        "nicht unterstützter Profilwert",
    ),
    (
        "profile counts and durations must be positive",
        "Anzahlen und Zeitspannen im Profil müssen positiv sein",
    ),
    (
        "profiles support only Host, None, ns:NAME or profile:NAME network defaults",
        "Profile unterstützen nur Host, None, ns:NAME oder profile:NAME als Netzwerkvorgabe",
    ),
    (
        "too many preset definitions",
        "zu viele Profil- und Klassendefinitionen",
    ),
    (
        "profile composition is too deep",
        "Profilverschachtelung ist zu tief",
    ),
    (
        "profile parent cannot be none",
        "übergeordnetes Profil darf nicht none sein",
    ),
    (
        "duplicate preset revision",
        "doppelte Profil- oder Klassenrevision",
    ),
    (
        "unknown preset revision",
        "unbekannte Profil- oder Klassenrevision",
    ),
    (
        "profile composition is cyclic or too deep",
        "Profilverschachtelung ist zyklisch oder zu tief",
    ),
    (
        "invalid preset registry",
        "ungültiges Profil- und Klassenregister",
    ),
    (
        "preset revision is immutable; use a new revision",
        "die Revision ist unveränderlich; verwende eine neue Revision",
    ),
    (
        "preset registry is full",
        "Profil- und Klassenregister ist voll",
    ),
    (
        "invalid preset snapshot",
        "ungültige gespeicherte Profilauflösung",
    ),
    (
        "usage: job profile|class list [--json]; show NAME@REV [--json]",
        "Aufruf: job profile|class list [--json]; show NAME@REV [--json]",
    ),
    (
        "Explicit versioned presets: --execution-profile NAME@REV|none; --class NAME@REV|none",
        "Explizite versionierte Vorgaben: --execution-profile NAME@REV|none; --class NAME@REV|none",
    ),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
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
    [
        "usage: job profile|class list [--json]; show NAME@REV [--json]",
        "Explicit versioned presets: --execution-profile NAME@REV|none; --class NAME@REV|none",
        "Collection defaults: --job-execution-profile NAME@REV|none; --job-class NAME@REV|none",
        "Classes set admission priority only. Direct Job fields override profile defaults; ancestor constraints still apply.",
    ]
    .map(message)
    .join("\n")
}
