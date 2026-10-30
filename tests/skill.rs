use std::fs;

fn repo_file(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

struct Frontmatter {
    keys: Vec<(String, String)>,
    metadata: Vec<(String, String)>,
}

fn split_skill(text: &str) -> Result<(Frontmatter, String), String> {
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return Err("unsupported line 1: the file must open with ---".to_string());
    }
    let mut keys = Vec::new();
    let mut metadata = Vec::new();
    let mut in_metadata = false;
    let mut seen_metadata = false;
    let mut closed = false;
    for (i, line) in lines.by_ref().enumerate() {
        let n = i + 2;
        if line == "---" {
            closed = true;
            break;
        }
        if let Some(rest) = line.strip_prefix("  ") {
            match rest.split_once(": ") {
                Some((k, v))
                    if in_metadata
                        && !k.is_empty()
                        && !k.starts_with(' ')
                        && v.len() >= 2
                        && v.starts_with('"')
                        && v.ends_with('"') =>
                {
                    metadata.push((k.to_string(), v[1..v.len() - 1].to_string()));
                }
                _ => return Err(format!("unsupported line {n}: {line:?}")),
            }
            continue;
        }
        if line == "metadata:" {
            if seen_metadata {
                return Err(format!("unsupported line {n}: second metadata block"));
            }
            seen_metadata = true;
            in_metadata = true;
            continue;
        }
        match line.split_once(": ") {
            Some((k, v)) if !k.is_empty() && !k.contains(' ') && !v.is_empty() => {
                if keys.iter().any(|(existing, _)| existing == k) {
                    return Err(format!("unsupported line {n}: repeated key {k:?}"));
                }
                in_metadata = false;
                keys.push((k.to_string(), v.to_string()));
            }
            _ => return Err(format!("unsupported line {n}: {line:?}")),
        }
    }
    if !closed {
        return Err("unsupported frontmatter: no closing ---".to_string());
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    Ok((Frontmatter { keys, metadata }, body))
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn check_against_spec(dir: &str, text: &str) -> Result<(), String> {
    let (fm, _) = split_skill(text)?;
    for (k, _) in &fm.keys {
        if !["name", "description", "license", "compatibility"].contains(&k.as_str()) {
            return Err(format!("frontmatter key {k:?} is not in the allowed keys"));
        }
    }
    let name = get(&fm.keys, "name").ok_or("name is missing")?;
    if name != dir {
        return Err(format!(
            "name {name:?} must equal the directory name {dir:?}"
        ));
    }
    if name.len() > 64 {
        return Err(format!("name {name:?} must be at most 64 characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("name {name:?} must use only [a-z0-9] and hyphens"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(format!("name {name:?} must not start or end with a hyphen"));
    }
    if name.contains("--") {
        return Err(format!("name {name:?} must use single hyphens"));
    }
    let description = get(&fm.keys, "description").ok_or("description is missing")?;
    if description.chars().count() > 1024 {
        return Err("description is longer than 1024 characters".to_string());
    }
    for needle in ["target", "cargo clean"] {
        if !description.contains(needle) {
            return Err(format!("description must contain {needle:?}"));
        }
    }
    if let Some(c) = get(&fm.keys, "compatibility") {
        if c.chars().count() > 500 {
            return Err("compatibility is longer than 500 characters".to_string());
        }
    }
    Ok(())
}

#[test]
fn skill_frontmatter_follows_the_spec() {
    let text = repo_file("skills/hive/SKILL.md");
    check_against_spec("hive", &text).unwrap();
    let (fm, body) = split_skill(&text).unwrap();
    assert_eq!(get(&fm.metadata, "hive-version"), Some("0.1.0"));
    assert!(!body.trim().is_empty(), "the skill body is empty");

    let ok_desc = "description: Reclaim a large target dir instead of cargo clean";
    let long = "x".repeat(1025);
    let long_name = "a".repeat(65);
    let long_compat = "y".repeat(501);
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "hive",
            format!("---\nname: other\n{ok_desc}\n---\nbody"),
            "directory name",
        ),
        (
            "hive-",
            format!("---\nname: hive-\n{ok_desc}\n---\nbody"),
            "start or end with a hyphen",
        ),
        (
            "-hive",
            format!("---\nname: -hive\n{ok_desc}\n---\nbody"),
            "start or end with a hyphen",
        ),
        (
            "hi_ve",
            format!("---\nname: hi_ve\n{ok_desc}\n---\nbody"),
            "[a-z0-9]",
        ),
        (
            "Hive",
            format!("---\nname: Hive\n{ok_desc}\n---\nbody"),
            "[a-z0-9]",
        ),
        (
            long_name.as_str(),
            format!("---\nname: {long_name}\n{ok_desc}\n---\nbody"),
            "at most 64 characters",
        ),
        (
            "hive",
            format!("---\nmetadata:\n  a: \"1\"\nname: hive\nmetadata:\n  b: \"2\"\n{ok_desc}\n---\nbody"),
            "second metadata",
        ),
        (
            "hive",
            format!("---\nname: hive\nname: hive\n{ok_desc}\n---\nbody"),
            "repeated key",
        ),
        (
            "hi--ve",
            format!("---\nname: hi--ve\n{ok_desc}\n---\nbody"),
            "single hyphens",
        ),
        (
            "hive",
            format!("---\nname: hive\ndescription: target cargo clean {long}\n---\nbody"),
            "1024",
        ),
        (
            "hive",
            "---\nname: hive\ndescription: cargo clean only\n---\nbody".to_string(),
            "\"target\"",
        ),
        (
            "hive",
            "---\nname: hive\ndescription: target only\n---\nbody".to_string(),
            "\"cargo clean\"",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\ncompatibility: {long_compat}\n---\nbody"),
            "500",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\nallowed-tools: Bash\n---\nbody"),
            "allowed keys",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\n- a list item\n---\nbody"),
            "unsupported line 4",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\nmetadata:\n  hive-version: 0.1.0\n---\nbody"),
            "unsupported line 5",
        ),
        (
            "hive",
            format!("name: hive\n{ok_desc}\n"),
            "unsupported line 1",
        ),
        (
            "hive",
            format!("---\nname: hive\n{ok_desc}\n"),
            "no closing ---",
        ),
    ];
    for (dir, crafted, rule) in &cases {
        let err = check_against_spec(dir, crafted)
            .expect_err(&format!("accepted a frontmatter that breaks {rule}"));
        assert!(err.contains(rule), "error {err:?} does not name {rule:?}");
    }
}
