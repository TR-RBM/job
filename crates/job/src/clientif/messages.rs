const CATALOGUE: &[(&str, &str)] = &[
    (
        "the greeting is not readable: write {\"hello\":{\"versions\":{\"min\":N,\"max\":N}}} with whole numbers from 1 and min not above max",
        "die Begrüßung ist nicht lesbar: schreib {\"hello\":{\"versions\":{\"min\":N,\"max\":N}}} mit ganzen Zahlen ab 1 und min nicht über max",
    ),
    (
        "the name of the client in the greeting is longer than 128 bytes",
        "der Name des Clients in der Begrüßung ist länger als 128 Bytes",
    ),
    (
        "the name of the client in the greeting is not a string",
        "der Name des Clients in der Begrüßung ist keine Zeichenkette",
    ),
    (
        "this user already has {limit} connections of the client interface open; close one and connect again",
        "dieser Benutzer hat schon {limit} Verbindungen der Client-Schnittstelle offen; schließ eine und verbinde dich neu",
    ),
    (
        "the line is longer than {limit} bytes; the connection is closed",
        "die Zeile ist länger als {limit} Bytes; die Verbindung wird geschlossen",
    ),
    (
        "the line is not a request: {detail}; write one JSON object with id and op on one line",
        "die Zeile ist keine Anfrage: {detail}; schreib ein JSON-Objekt mit id und op in eine Zeile",
    ),
    ("it is not valid UTF-8", "sie ist kein gültiges UTF-8"),
    ("it is not a JSON object", "sie ist kein JSON-Objekt"),
    (
        "op is missing or not a string",
        "op fehlt oder ist keine Zeichenkette",
    ),
    (
        "id is missing or not a number",
        "id fehlt oder ist keine Zahl",
    ),
    (
        "this service does not know the request `{op}`; the capabilities in the greeting list what it answers",
        "dieser Dienst kennt die Anfrage `{op}` nicht; die Fähigkeiten in der Begrüßung nennen, was er beantwortet",
    ),
    ("args is not an object", "args ist kein Objekt"),
    (
        "the answer would be longer than {limit} bytes; ask for less",
        "die Antwort wäre länger als {limit} Bytes; frag nach weniger",
    ),
    (
        "client interface: {client} connected as user {uid}, process {pid}, version {version}",
        "Client-Schnittstelle: {client} verbunden als Benutzer {uid}, Prozess {pid}, Version {version}",
    ),
    ("language is `en` or `de`", "language ist `en` oder `de`"),
    (
        "command needs words: an array with at least one string",
        "command braucht words: ein Array mit mindestens einer Zeichenkette",
    ),
    ("cwd is not an absolute path", "cwd ist kein absoluter Pfad"),
    (
        "`{word}` needs cwd, an absolute path on the service's host",
        "`{word}` braucht cwd, einen absoluten Pfad auf dem Rechner des Dienstes",
    ),
    (
        "env is not an object {\"vars\":[[\"NAME\",\"value\"],...]}",
        "env ist kein Objekt {\"vars\":[[\"NAME\",\"value\"],...]}",
    ),
    (
        "terminal is not an object {\"rows\":R,\"cols\":C} with 1 to 200 rows and 1 to 500 columns",
        "terminal ist kein Objekt {\"rows\":R,\"cols\":C} mit 1 bis 200 Zeilen und 1 bis 500 Spalten",
    ),
    ("session is not a string", "session ist keine Zeichenkette"),
    (
        "dry_run is not true or false",
        "dry_run ist nicht true oder false",
    ),
    (
        "--stdin cannot be carried: the standard input of the client is not on this connection",
        "--stdin lässt sich nicht übertragen: die Standardeingabe des Clients liegt nicht auf dieser Verbindung",
    ),
    (
        "--current-env is not accepted here: the service has no environment of the client; give the environment in env",
        "--current-env wird hier nicht angenommen: der Dienst hat keine Umgebung des Clients; gib die Umgebung in env an",
    ),
    (
        "job {command} has no --dry-run, so dry_run is not accepted for it",
        "job {command} hat kein --dry-run, also wird dry_run dafür nicht angenommen",
    ),
    (
        "the saved environment of Job {id} cannot be read, so it was not edited; give the environment in env",
        "die gespeicherte Umgebung von Job {id} lässt sich nicht lesen, also wurde er nicht bearbeitet; gib die Umgebung in env an",
    ),
    (
        "the service failed while it carried out the command; its log says more",
        "der Dienst ist beim Ausführen des Befehls gescheitert; sein Protokoll sagt mehr",
    ),
    (
        "the request gives no cwd, and this command needs one",
        "die Anfrage nennt kein cwd, und dieser Befehl braucht eines",
    ),
    (
        "native needs request: one request of the internal protocol",
        "native braucht request: eine Anfrage des internen Protokolls",
    ),
    (
        "native does not carry the internal version envelope; send the request itself",
        "native überträgt den internen Versionsumschlag nicht; schick die Anfrage selbst",
    ),
    (
        "native does not carry the internal output stream; use the request output",
        "native überträgt den internen Ausgabestrom nicht; nimm die Anfrage output",
    ),
    (
        "native carries reading requests only; send a change as the words of a job command with the request command",
        "native überträgt nur lesende Anfragen; schick eine Änderung als die Wörter eines job-Befehls mit der Anfrage command",
    ),
    (
        "the service could not write its own answer: {error}",
        "der Dienst konnte seine eigene Antwort nicht schreiben: {error}",
    ),
    (
        "`{member}` in the arguments of {op} must be {wanted}",
        "`{member}` in den Argumenten von {op} muss {wanted} sein",
    ),
    (
        "a whole number that is not negative",
        "eine ganze Zahl, die nicht negativ ist",
    ),
    ("a string", "eine Zeichenkette"),
    ("true or false", "true oder false"),
    ("an object", "ein Objekt"),
    ("an array of strings", "eine Liste von Zeichenketten"),
    ("an object of strings", "ein Objekt aus Zeichenketten"),
    (
        "a string of at most 256 bytes",
        "eine Zeichenkette von höchstens 256 Bytes",
    ),
    (
        "{op} needs `{member}`: a whole number",
        "{op} braucht `{member}`: eine ganze Zahl",
    ),
    (
        "`{member}` in the arguments of {op} must be one of {words}",
        "`{member}` in den Argumenten von {op} muss eines von {words} sein",
    ),
    (
        "{op} takes `{first}` or `{second}`, not both",
        "{op} nimmt `{first}` oder `{second}`, nicht beides",
    ),
    (
        "`{member}` in the filter of jobs must be {wanted}",
        "`{member}` im Filter von jobs muss {wanted} sein",
    ),
    (
        "`states` in the filter of jobs holds {given}, which is not a state; the states are {states}",
        "`states` im Filter von jobs enthält {given}, das ist kein Zustand; die Zustände sind {states}",
    ),
    (
        "the filter of jobs has no member `{member}`; it takes states, subtree, labels, actor_uid, session and text",
        "der Filter von jobs kennt `{member}` nicht; er nimmt states, subtree, labels, actor_uid, session und text",
    ),
    (
        "`cursor` in the arguments of jobs is not a cursor of this service; send one from an earlier answer unchanged",
        "`cursor` in den Argumenten von jobs ist kein Cursor dieses Dienstes; schick einen aus einer früheren Antwort unverändert",
    ),
    (
        "the cursor belongs to another order or filter; send it with the order and filter of the answer it came from, or ask without a cursor",
        "der Cursor gehört zu einer anderen Reihenfolge oder einem anderen Filter; schick ihn mit der Reihenfolge und dem Filter der Antwort, aus der er stammt, oder frag ohne Cursor",
    ),
    (
        "`limit` in the arguments of {op} is above {limit}, the most rows a page holds",
        "`limit` in den Argumenten von {op} liegt über {limit}, so viele Zeilen fasst eine Seite höchstens",
    ),
    (
        "`direction` in the arguments of jobs goes with a cursor only; leave it out, or send the cursor of an earlier answer",
        "`direction` in den Argumenten von jobs gilt nur mit einem Cursor; lass es weg oder schick den Cursor einer früheren Antwort",
    ),
    (
        "`from` in the arguments of output must be \"start\", \"end\" or {\"sequence\":N} with a whole number N",
        "`from` in den Argumenten von output muss \"start\", \"end\" oder {\"sequence\":N} mit einer ganzen Zahl N sein",
    ),
    (
        "`streams` in the arguments of output must be an array of {streams}",
        "`streams` in den Argumenten von output muss eine Liste aus {streams} sein",
    ),
    (
        "output follows forward only; leave out `follow` or ask with direction forward",
        "output folgt nur vorwärts; lass `follow` weg oder frag mit direction forward",
    ),
    (
        "`max_bytes` in the arguments of output must be from 1 to {limit}",
        "`max_bytes` in den Argumenten von output muss zwischen 1 und {limit} liegen",
    ),
    (
        "the recorded output of Job {id} cannot be read: {error}",
        "die aufgezeichnete Ausgabe von Job {id} lässt sich nicht lesen: {error}",
    ),
    (
        "end needs `of`: the id of the output request to stop",
        "end braucht `of`: die id der output-Anfrage, die enden soll",
    ),
    (
        "`{member}` must be an array of at most {limit} Job IDs",
        "`{member}` muss eine Liste von höchstens {limit} Job-IDs sein",
    ),
    (
        "this user already has {limit} subscriptions; end one, or use its connection",
        "dieser Benutzer hat schon {limit} Abonnements; beende eins oder nutze dessen Verbindung",
    ),
    (
        "interest needs `jobs`: an array of Job IDs, which replaces the set",
        "interest braucht `jobs`: eine Liste von Job-IDs, die die Menge ersetzt",
    ),
    (
        "`after` in the arguments of subscribe must be {\"started_ms\":S,\"seq\":N} with whole numbers",
        "`after` in den Argumenten von subscribe muss {\"started_ms\":S,\"seq\":N} mit ganzen Zahlen sein",
    ),
    (
        "`rows` in the arguments of subscribe must be an object with order, filter and limit",
        "`rows` in den Argumenten von subscribe muss ein Objekt mit order, filter und limit sein",
    ),
    (
        "output is not accepted on a connection that has a subscription; use a second connection",
        "output wird auf einer Verbindung mit Abonnement nicht angenommen; nimm eine zweite Verbindung",
    ),
    (
        "this connection has a subscription already; a connection carries one",
        "diese Verbindung hat schon ein Abonnement; eine Verbindung trägt genau eins",
    ),
    (
        "interest needs a subscription on the same connection; send subscribe first",
        "interest braucht ein Abonnement auf derselben Verbindung; schick zuerst subscribe",
    ),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(english, _)| *english == key)
            .map_or(key, |(_, german)| *german)
            .to_owned()
    } else {
        key.to_owned()
    }
}
