const CATALOGUE: &[(&str, &str)] = &[
    (
        "--net-secret-file: cannot read {path}: {error}",
        "--net-secret-file: {path} lässt sich nicht lesen: {error}",
    ),
    (
        "--net-secret-file: {path} is not a regular file",
        "--net-secret-file: {path} ist keine gewöhnliche Datei",
    ),
    (
        "--net-secret-file: {path} belongs to another user; it must belong to the user the job service runs as",
        "--net-secret-file: {path} gehört einem anderen Benutzer; die Datei muss dem Benutzer gehören, unter dem der Job-Dienst läuft",
    ),
    (
        "--net-secret-file: {path} can be read by its group or by others; run: chmod 600 {path}",
        "--net-secret-file: {path} ist für die Gruppe oder für andere lesbar; führe aus: chmod 600 {path}",
    ),
    (
        "--net-secret-file: {path} must hold one line USER:PASSWORD; write @, / and spaces percent-encoded",
        "--net-secret-file: {path} muss eine Zeile USER:PASSWORT enthalten; schreibe @, / und Leerzeichen prozentkodiert",
    ),
    (
        "--net-secret-file belongs to a proxy; give --net socks5://HOST:PORT, http://HOST:PORT or https://HOST:PORT with it",
        "--net-secret-file gehört zu einem Proxy; gib dazu --net socks5://HOST:PORT, http://HOST:PORT oder https://HOST:PORT an",
    ),
    (
        "give the proxy's user and password either inside --net or in --net-secret-file, not both",
        "gib Benutzer und Passwort des Proxys entweder in --net oder in --net-secret-file an, nicht in beiden",
    ),
    (
        "{option}: {path} must be an absolute path",
        "{option}: {path} muss ein absoluter Pfad sein",
    ),
    (
        "--net: `{net}` is not a network; write default, none, socks5://HOST:PORT, http://HOST:PORT, https://HOST:PORT, wireguard:FILE, openvpn:FILE, ns:NAME or profile:NAME",
        "--net: `{net}` ist kein Netzwerk; schreibe default, none, socks5://HOST:PORT, http://HOST:PORT, https://HOST:PORT, wireguard:DATEI, openvpn:DATEI, ns:NAME oder profile:NAME",
    ),
    (
        "--net: write the proxy's user and password as USER:PASSWORD before the @, with @, / and spaces percent-encoded",
        "--net: schreibe Benutzer und Passwort des Proxys als USER:PASSWORT vor das @, mit @, / und Leerzeichen prozentkodiert",
    ),
    (
        "the jobs already on network {name} could not be kept apart from each other: {error}; no further job joins it until that works",
        "die Jobs, die schon im Netzwerk {name} sind, ließen sich nicht voneinander trennen: {error}; bis das gelingt, kommt kein weiterer Job dazu",
    ),
    (
        "cannot save network {name} without its proxy password: {error}",
        "das Netzwerk {name} lässt sich nicht ohne sein Proxy-Passwort speichern: {error}",
    ),
    (
        "cannot keep the proxy's user and password in {path}: {error}",
        "Benutzer und Passwort des Proxys lassen sich nicht in {path} ablegen: {error}",
    ),
    (
        "Network secret: --net-secret-file FILE holds USER:PASSWORD for the proxy named in --net; the file belongs to the service user and has mode 600. A Queue or Group takes the same option beside --net.",
        "Netzwerk-Geheimnis: --net-secret-file DATEI enthält USER:PASSWORT für den Proxy aus --net; die Datei gehört dem Dienstbenutzer und hat den Modus 600. Eine Queue oder Gruppe nimmt dieselbe Option neben --net.",
    ),
    (
        "A proxy's user and password are shown as *** everywhere. The job itself receives them in its proxy variables; other clients of the service do not. Setting proxy variables alone isolates nothing: the job's own network namespace does.",
        "Benutzer und Passwort eines Proxys erscheinen überall als ***. Der Job selbst erhält sie in seinen Proxy-Variablen; andere Clients des Dienstes nicht. Proxy-Variablen allein schotten nichts ab: Das leistet der eigene Netzwerk-Namensraum des Jobs.",
    ),
];

pub fn message(english: &str, values: &[(&str, String)]) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    let template = if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, german)| *german)
    } else {
        english
    };
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{key}}}"), value)
        })
}

pub fn help() -> String {
    [
        "Network secret: --net-secret-file FILE holds USER:PASSWORD for the proxy named in --net; the file belongs to the service user and has mode 600. A Queue or Group takes the same option beside --net.",
        "A proxy's user and password are shown as *** everywhere. The job itself receives them in its proxy variables; other clients of the service do not. Setting proxy variables alone isolates nothing: the job's own network namespace does.",
    ]
    .map(|line| message(line, &[]))
    .join("\n")
}
