const CATALOGUE: &[(&str, &str)] = &[
    (
        "`{name}` is not a name: use up to 64 letters, digits, - and _",
        "`{name}` ist kein Name: verwende bis zu 64 Buchstaben, Ziffern, - und _",
    ),
    (
        "the network namespace {name}",
        "den Netzwerk-Namespace {name}",
    ),
    ("the network profile {name}", "das Netzwerkprofil {name}"),
    ("user namespaces", "User-Namespaces"),
    (
        "network namespace {name}: {field} = {path} must be an absolute path without `..` and outside /proc; a path under /proc names a process, not a namespace",
        "Netzwerk-Namespace {name}: {field} = {path} muss ein absoluter Pfad ohne `..` und außerhalb von /proc sein; ein Pfad unter /proc benennt einen Prozess, keinen Namespace",
    ),
    (
        "network namespace {name}: resolv_conf = {path} must be an absolute path",
        "Netzwerk-Namespace {name}: resolv_conf = {path} muss ein absoluter Pfad sein",
    ),
    (
        "there is no network namespace {name} in the service configuration; `job net list` shows what it names",
        "in der Dienstkonfiguration gibt es keinen Netzwerk-Namespace {name}; `job net list` zeigt, was sie nennt",
    ),
    (
        "there is no network profile {name} in the service configuration; `job net list` shows what it names",
        "in der Dienstkonfiguration gibt es kein Netzwerkprofil {name}; `job net list` zeigt, was sie nennt",
    ),
    ("network profile {name}", "Netzwerkprofil {name}"),
    (
        "at most 3 dns addresses and 64 allow and 64 deny rules",
        "höchstens 3 dns-Adressen sowie 64 allow- und 64 deny-Regeln",
    ),
    (
        "{field} belongs to a proxy; egress {egress} takes no user and password",
        "{field} gehört zu einem Proxy; egress {egress} nimmt keinen Benutzer und kein Passwort",
    ),
    (
        "egress carries a user and password; put them in a file and name it with secret_file",
        "egress enthält Benutzer und Passwort; lege sie in eine Datei und nenne sie mit secret_file",
    ),
    (
        "{field} cannot be set with egress {egress}: names are resolved by the proxy, and the Job reaches nothing but the proxy",
        "{field} lässt sich mit egress {egress} nicht setzen: Namen löst der Proxy auf, und der Job erreicht nichts außer dem Proxy",
    ),
    (
        "{field} cannot be set with egress {egress}: the Job reaches nothing but the proxy, so filter destinations at the proxy",
        "{field} lässt sich mit egress {egress} nicht setzen: der Job erreicht nichts außer dem Proxy, filtere Ziele also am Proxy",
    ),
    (
        "{field} cannot be set with egress {egress}: there is no network to shape, filter or resolve names on",
        "{field} lässt sich mit egress {egress} nicht setzen: es gibt kein Netzwerk, das sich begrenzen oder filtern ließe oder in dem Namen aufgelöst würden",
    ),
    (
        "{field} cannot be set with egress {egress}: job does not own that namespace's devices, filter or resolver",
        "{field} lässt sich mit egress {egress} nicht setzen: job gehören weder die Geräte noch der Filter noch der Resolver dieses Namespace",
    ),
    (
        "openvpn: not yet; use wireguard:FILE or a proxy",
        "openvpn: noch nicht; nimm wireguard:DATEI oder einen Proxy",
    ),
    (
        "egress cannot name another profile",
        "egress kann kein anderes Profil nennen",
    ),
    (
        "egress wireguard:FILE needs an absolute path",
        "egress wireguard:DATEI braucht einen absoluten Pfad",
    ),
    (
        "job_bandwidth is wider than bandwidth",
        "job_bandwidth ist größer als bandwidth",
    ),
    (
        "job_bandwidth without bandwidth needs sharing = \"per-job\"; a shared network holds its Jobs within one bandwidth",
        "job_bandwidth ohne bandwidth braucht sharing = \"per-job\"; ein geteiltes Netzwerk hält seine Jobs innerhalb einer Bandbreite",
    ),
    (
        "at most 64 network namespaces and 64 network profiles",
        "höchstens 64 Netzwerk-Namespaces und 64 Netzwerkprofile",
    ),
    (
        "namespace_directory = {path} must be an absolute path without `..` and outside /proc",
        "namespace_directory = {path} muss ein absoluter Pfad ohne `..` und außerhalb von /proc sein",
    ),
    (
        "{path} is set to {net}: {error}",
        "{path} ist auf {net} gesetzt: {error}",
    ),
    (
        "--net-secret-file cannot be set beside {net}: a profile names its own secret_file, and a namespace takes none",
        "--net-secret-file lässt sich neben {net} nicht setzen: ein Profil nennt sein eigenes secret_file, und ein Namespace nimmt keines",
    ),
    (
        "a Queue's bandwidth cannot be set beside {net}: a profile sets its own bandwidth, and job does not own the devices of a namespace it joins",
        "die Bandbreite einer Queue lässt sich neben {net} nicht setzen: ein Profil setzt seine eigene Bandbreite, und job gehören die Geräte eines Namespace nicht, dem es beitritt",
    ),
    (
        "network namespace {name}: cannot open {path}: {error}",
        "Netzwerk-Namespace {name}: {path} lässt sich nicht öffnen: {error}",
    ),
    (
        "network namespace {name}: {path} is not a namespace file",
        "Netzwerk-Namespace {name}: {path} ist keine Namespace-Datei",
    ),
    (
        "network namespace {name}: {path} is a namespace, but not a {kind} namespace",
        "Netzwerk-Namespace {name}: {path} ist ein Namespace, aber kein {kind}-Namespace",
    ),
    (
        "network namespace {name}: resolv_conf = {path} is not a file",
        "Netzwerk-Namespace {name}: resolv_conf = {path} ist keine Datei",
    ),
    (
        "network namespace {name}: the service may not join the user namespace named in user_namespace: it was made by another user, or the service is not in its parent ({error})",
        "Netzwerk-Namespace {name}: der Dienst darf dem in user_namespace genannten User-Namespace nicht beitreten: ein anderer Benutzer hat ihn angelegt, oder der Dienst ist nicht in dessen Eltern-Namespace ({error})",
    ),
    (
        "network namespace {name}: cannot join the user namespace named in user_namespace: {error}",
        "Netzwerk-Namespace {name}: Beitritt zum in user_namespace genannten User-Namespace gelingt nicht: {error}",
    ),
    (
        "network namespace {name}: the service may not join this namespace: it belongs to a user namespace the service is not in; run the service with the needed privilege or give user_namespace ({error})",
        "Netzwerk-Namespace {name}: der Dienst darf diesem Namespace nicht beitreten: er gehört zu einem User-Namespace, in dem der Dienst nicht ist; starte den Dienst mit dem nötigen Recht oder gib user_namespace an ({error})",
    ),
    (
        "network namespace {name}: the service may not join this namespace: it does not belong to the user namespace named in user_namespace ({error})",
        "Netzwerk-Namespace {name}: der Dienst darf diesem Namespace nicht beitreten: er gehört nicht zu dem in user_namespace genannten User-Namespace ({error})",
    ),
    (
        "network namespace {name}: cannot join it: {error}",
        "Netzwerk-Namespace {name}: Beitritt gelingt nicht: {error}",
    ),
    (
        "network namespace {name}: after joining, the process was not in the namespace the configuration names ({error})",
        "Netzwerk-Namespace {name}: nach dem Beitritt war der Prozess nicht in dem Namespace, den die Konfiguration nennt ({error})",
    ),
    (
        "network namespace {name}: cannot put resolv_conf over /etc/resolv.conf: {error}",
        "Netzwerk-Namespace {name}: resolv_conf lässt sich nicht über /etc/resolv.conf legen: {error}",
    ),
    (
        "network namespace {name}: cannot return to the service's user inside the namespace: {error}",
        "Netzwerk-Namespace {name}: Rückkehr zum Benutzer des Dienstes im Namespace gelingt nicht: {error}",
    ),
    (
        "queue {queue}'s jobs have {theirs}; this job asks for {mine}; run it outside the queue",
        "die Jobs der Queue {queue} haben {theirs}; dieser Job verlangt {mine}; starte ihn außerhalb der Queue",
    ),
    (
        "--bandwidth cannot be used with ns:{name}: job does not own that namespace's devices, so it cannot shape them",
        "--bandwidth lässt sich mit ns:{name} nicht verwenden: job gehören die Geräte dieses Namespace nicht, also kann es sie nicht begrenzen",
    ),
    (
        "--net-secret-file cannot be used with profile:{name}: the profile names its own secret_file",
        "--net-secret-file lässt sich mit profile:{name} nicht verwenden: das Profil nennt sein eigenes secret_file",
    ),
    (
        "--net-secret-file belongs to a proxy; profile:{name} does not lead through one",
        "--net-secret-file gehört zu einem Proxy; profile:{name} führt durch keinen",
    ),
    (
        "--bandwidth cannot be used with profile:{name}: its network is not one job can shape",
        "--bandwidth lässt sich mit profile:{name} nicht verwenden: sein Netzwerk kann job nicht begrenzen",
    ),
    (
        "--bandwidth {asked} is wider than the {limit} profile:{name} allows a Job",
        "--bandwidth {asked} ist mehr als die {limit}, die profile:{name} einem Job erlaubt",
    ),
    (
        "--bandwidth cannot be used with profile:{name}: its network is shared and the profile sets no bandwidth to divide",
        "--bandwidth lässt sich mit profile:{name} nicht verwenden: sein Netzwerk ist geteilt, und das Profil setzt keine Bandbreite, die sich aufteilen ließe",
    ),
    (
        "this host cannot enforce profile:{name}: it lacks {missing}",
        "dieser Host kann profile:{name} nicht durchsetzen: ihm fehlt {missing}",
    ),
    (
        "{error}; it was there when the Job was submitted, and the Job cannot start without it",
        "{error}; beim Einreichen des Jobs war es vorhanden, und ohne es kann der Job nicht starten",
    ),
    (
        "the host's network, shared with everything on the host",
        "das Netzwerk des Hosts, geteilt mit allem auf dem Host",
    ),
    ("one network per Job", "ein Netzwerk je Job"),
    (
        "one exit and rate budget per Queue; each Job in a network namespace of its own",
        "ein Ausgang und ein Bandbreitenbudget je Queue; jeder Job in einem eigenen Netzwerk-Namespace",
    ),
    (
        "one exit and rate budget for all Jobs of the profile; each Job in a network namespace of its own",
        "ein Ausgang und ein Bandbreitenbudget für alle Jobs des Profils; jeder Job in einem eigenen Netzwerk-Namespace",
    ),
    (
        "an existing network namespace, shared with every Job that selects it and with whatever else is in it",
        "ein vorhandener Netzwerk-Namespace, geteilt mit jedem Job, der ihn wählt, und mit allem, was sonst darin ist",
    ),
    ("joinable", "beitretbar"),
    ("not joinable", "nicht beitretbar"),
    ("enforceable", "durchsetzbar"),
    ("not enforceable here", "hier nicht durchsetzbar"),
    ("description", "Beschreibung"),
    ("network", "Netzwerk"),
    (
        "the service configuration names no network namespace and no network profile",
        "die Dienstkonfiguration nennt keinen Netzwerk-Namespace und kein Netzwerkprofil",
    ),
    (
        "job adds no filter, no name resolution and no bandwidth limit to this namespace",
        "job fügt diesem Namespace keinen Filter, keine Namensauflösung und keine Bandbreitengrenze hinzu",
    ),
    (
        "packet filter, in order:",
        "Paketfilter, in dieser Reihenfolge:",
    ),
    (
        "usage: job net list [--json] | show NAME [--json]",
        "Aufruf: job net list [--json] | show NAME [--json]",
    ),
    (
        "the service configuration names no network namespace and no network profile {name}; `job net list` shows what it names",
        "die Dienstkonfiguration nennt keinen Netzwerk-Namespace und kein Netzwerkprofil {name}; `job net list` zeigt, was sie nennt",
    ),
    (
        "{name} is both a network namespace and a network profile; write ns:{name} or profile:{name}",
        "{name} ist sowohl ein Netzwerk-Namespace als auch ein Netzwerkprofil; schreibe ns:{name} oder profile:{name}",
    ),
    (
        "`{address}` is an IPv6 address; these networks carry IPv4 only",
        "`{address}` ist eine IPv6-Adresse; diese Netzwerke tragen nur IPv4",
    ),
    (
        "`{address}` is not an IPv4 address",
        "`{address}` ist keine IPv4-Adresse",
    ),
    (
        "`{rule}` names an IPv6 range; these networks carry IPv4 only, so write IPv4 ranges",
        "`{rule}` nennt einen IPv6-Bereich; diese Netzwerke tragen nur IPv4, schreibe also IPv4-Bereiche",
    ),
    (
        "`{rule}`: write a port as a number from 1 to 65535, or FIRST-LAST",
        "`{rule}`: schreibe einen Port als Zahl von 1 bis 65535 oder als ERSTER-LETZTER",
    ),
    (
        "`{rule}`: write ADDRESS[/PREFIX][:PORT[-PORT]][/tcp|/udp] with an IPv4 address",
        "`{rule}`: schreibe ADRESSE[/PRÄFIX][:PORT[-PORT]][/tcp|/udp] mit einer IPv4-Adresse",
    ),
    (
        "network namespace {name}: joinable",
        "Netzwerk-Namespace {name}: beitretbar",
    ),
    (
        "prepare the namespace or correct [network.namespaces] in the service configuration; Jobs that do not select it are not affected",
        "bereite den Namespace vor oder korrigiere [network.namespaces] in der Dienstkonfiguration; Jobs, die ihn nicht wählen, sind nicht betroffen",
    ),
    (
        "install what is missing or correct [network.profiles] in the service configuration; Jobs that do not select the profile are not affected",
        "installiere, was fehlt, oder korrigiere [network.profiles] in der Dienstkonfiguration; Jobs, die das Profil nicht wählen, sind nicht betroffen",
    ),
    (
        "Network selection: --net ns:NAME joins a network namespace the administrator named under [network.namespaces] in the service configuration; --net profile:NAME applies a routing profile from [network.profiles]. A client can select only what the configuration names.",
        "Netzwerkwahl: --net ns:NAME tritt einem Netzwerk-Namespace bei, den der Administrator unter [network.namespaces] in der Dienstkonfiguration benannt hat; --net profile:NAME wendet ein Routing-Profil aus [network.profiles] an. Ein Client kann nur wählen, was die Konfiguration nennt.",
    ),
    (
        "job net list shows the configured names; job net show NAME shows one with its rules, its sharing and whether this host can enforce it. Every Job that selects ns:NAME shares that namespace; job adds no filter, name resolution or bandwidth limit there.",
        "job net list zeigt die konfigurierten Namen; job net show NAME zeigt einen mit seinen Regeln, seiner Teilung und ob dieser Host ihn durchsetzen kann. Jeder Job, der ns:NAME wählt, teilt diesen Namespace; job fügt dort keinen Filter, keine Namensauflösung und keine Bandbreitengrenze hinzu.",
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
        "Network selection: --net ns:NAME joins a network namespace the administrator named under [network.namespaces] in the service configuration; --net profile:NAME applies a routing profile from [network.profiles]. A client can select only what the configuration names.",
        "job net list shows the configured names; job net show NAME shows one with its rules, its sharing and whether this host can enforce it. Every Job that selects ns:NAME shares that namespace; job adds no filter, name resolution or bandwidth limit there.",
        "usage: job net list [--json] | show NAME [--json]",
    ]
    .map(|line| message(line, &[]))
    .join("\n")
}
