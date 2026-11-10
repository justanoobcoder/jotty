//! TOML values written as Nix expressions, for `programs.jotty.settings`.

/// The Nix expression for `value`, indented as a top-level expression.
/// Fails on infinite and NaN floats, which Nix can't spell.
pub fn to_nix(value: &toml::Value) -> Result<String, String> {
    let mut out = String::new();
    write_value(&mut out, value, 0)?;
    out.push('\n');
    Ok(out)
}

fn write_value(out: &mut String, value: &toml::Value, depth: usize) -> Result<(), String> {
    use std::fmt::Write;
    match value {
        toml::Value::String(s) => write_string(out, s),
        toml::Value::Integer(i) => {
            let _ = write!(out, "{i}");
        }
        toml::Value::Float(f) => {
            if !f.is_finite() {
                return Err(format!("{f} has no Nix equivalent"));
            }
            // `{:?}` keeps a decimal point (`13.0`) so Nix reads a float,
            // except in exponents (`1e21`), which Nix needs as `1.0e21`.
            let text = format!("{f:?}");
            match text.split_once('e') {
                Some((mantissa, exp)) if !mantissa.contains('.') => {
                    let _ = write!(out, "{mantissa}.0e{exp}");
                }
                _ => out.push_str(&text),
            }
        }
        toml::Value::Boolean(b) => {
            let _ = write!(out, "{b}");
        }
        toml::Value::Datetime(d) => write_string(out, &d.to_string()),
        toml::Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[ ]");
                return Ok(());
            }
            out.push('[');
            for item in items {
                out.push(' ');
                // A negative number would be read as a subtraction.
                let negative = matches!(item, toml::Value::Integer(i) if *i < 0)
                    || matches!(item, toml::Value::Float(f) if f.is_sign_negative());
                if negative {
                    out.push('(');
                }
                write_value(out, item, depth)?;
                if negative {
                    out.push(')');
                }
            }
            out.push_str(" ]");
        }
        toml::Value::Table(table) => {
            if table.is_empty() {
                out.push_str("{ }");
                return Ok(());
            }
            out.push_str("{\n");
            for (key, value) in table {
                indent(out, depth + 1);
                write_key(out, key);
                out.push_str(" = ");
                write_value(out, value, depth + 1)?;
                out.push_str(";\n");
            }
            indent(out, depth);
            out.push('}');
        }
    }
    Ok(())
}

fn indent(out: &mut String, depth: usize) {
    out.extend(std::iter::repeat_n("  ", depth));
}

fn write_key(out: &mut String, key: &str) {
    if is_identifier(key) {
        out.push_str(key);
    } else {
        write_string(out, key);
    }
}

/// Whether `key` can be an attribute name without quotes.
fn is_identifier(key: &str) -> bool {
    const KEYWORDS: [&str; 10] = [
        "assert", "else", "if", "in", "inherit", "let", "or", "rec", "then", "with",
    ];
    let mut chars = key.chars();
    let starts_well = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    starts_well
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '\'' | '-'))
        && !KEYWORDS.contains(&key)
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // `${` starts an interpolation; a lone `$` is fine.
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nix(toml: &str) -> String {
        to_nix(&toml::Value::Table(toml::from_str(toml).unwrap())).unwrap()
    }

    #[test]
    fn nested_tables_become_attrsets() {
        assert_eq!(
            nix("[font]\nsize = 13.0\nfallback = []\n[window]\nblur = true\npadding = 8"),
            "{\n  font = {\n    fallback = [ ];\n    size = 13.0;\n  };\n  window = {\n    blur = true;\n    padding = 8;\n  };\n}\n"
        );
        assert_eq!(nix(""), "{ }\n");
    }

    #[test]
    fn floats_keep_a_decimal_point() {
        assert_eq!(
            nix("a = 13.0\nb = 0.85"),
            "{\n  a = 13.0;\n  b = 0.85;\n}\n"
        );
        assert!(to_nix(&toml::Value::Float(f64::NAN)).is_err());
        let float = |f| to_nix(&toml::Value::Float(f)).unwrap();
        assert_eq!(float(1e21), "1.0e21\n");
        assert_eq!(float(1.5e-7), "1.5e-7\n");
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(
            nix(r#"a = "say \"hi\" \\ ${HOME} $x""#),
            "{\n  a = \"say \\\"hi\\\" \\\\ \\${HOME} $x\";\n}\n"
        );
    }

    #[test]
    fn odd_keys_are_quoted() {
        assert_eq!(
            nix("[keybinds]\n\"ctrl+c\" = 1\ncopy_or_interrupt = [\"ctrl+c\"]\nin = 2"),
            "{\n  keybinds = {\n    copy_or_interrupt = [ \"ctrl+c\" ];\n    \"ctrl+c\" = 1;\n    \"in\" = 2;\n  };\n}\n"
        );
    }

    #[test]
    fn negative_list_items_are_parenthesized() {
        assert_eq!(nix("a = [1, -2, -0.5]"), "{\n  a = [ 1 (-2) (-0.5) ];\n}\n");
    }
}
