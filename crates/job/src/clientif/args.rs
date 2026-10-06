use serde_json::{Map, Value};

use super::message;

pub type Args = Map<String, Value>;

fn wrong(op: &str, member: &str, wanted: &str) -> String {
    message("`{member}` in the arguments of {op} must be {wanted}")
        .replace("{member}", member)
        .replace("{op}", op)
        .replace("{wanted}", &message(wanted))
}

pub fn number(args: &Args, op: &str, member: &str) -> Result<Option<u64>, String> {
    match args.get(member) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| wrong(op, member, "a whole number that is not negative")),
    }
}

pub fn needed(args: &Args, op: &str, member: &str) -> Result<u64, String> {
    number(args, op, member)?.ok_or_else(|| {
        message("{op} needs `{member}`: a whole number")
            .replace("{op}", op)
            .replace("{member}", member)
    })
}

pub fn text<'a>(args: &'a Args, op: &str, member: &str) -> Result<Option<&'a str>, String> {
    match args.get(member) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(wrong(op, member, "a string")),
    }
}

pub fn flag(args: &Args, op: &str, member: &str) -> Result<bool, String> {
    match args.get(member) {
        None => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(wrong(op, member, "true or false")),
    }
}

pub fn word<'a>(
    args: &'a Args,
    op: &str,
    member: &str,
    words: &[&'a str],
) -> Result<Option<&'a str>, String> {
    match text(args, op, member)? {
        None => Ok(None),
        Some(given) => words
            .iter()
            .find(|word| **word == given)
            .copied()
            .map(Some)
            .ok_or_else(|| {
                message("`{member}` in the arguments of {op} must be one of {words}")
                    .replace("{member}", member)
                    .replace("{op}", op)
                    .replace("{words}", &words.join(", "))
            }),
    }
}

pub fn together(op: &str, first: &str, second: &str) -> String {
    message("{op} takes `{first}` or `{second}`, not both")
        .replace("{op}", op)
        .replace("{first}", first)
        .replace("{second}", second)
}
