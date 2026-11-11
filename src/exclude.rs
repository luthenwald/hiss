//! Files hiss never sends: those that commonly hold secrets, by Helix's
//! language name or by file name, and whatever the `exclude` setting adds.
//!
//! A pattern without a slash matches the file name, one with a slash the
//! whole path (`~/` stands for the home folder). `*` matches any run of
//! characters, `?` one.

use std::path::Path;

/// Languages Helix gives to files of secrets and system accounts.
const LANGUAGES: [&str; 5] = ["env", "pem", "passwd", "hosts", "ssh_client_config"];

/// File names of keys, tokens and passwords.
const NAMES: [&str; 18] = [
   ".env",
   ".env.*",
   "*.env",
   ".envrc",
   "*.pem",
   "*.key",
   "id_rsa*",
   "id_dsa*",
   "id_ecdsa*",
   "id_ed25519*",
   ".netrc",
   ".npmrc",
   ".pypirc",
   ".pgpass",
   ".htpasswd",
   ".git-credentials",
   "credentials",
   "*credentials.json",
];

/// Whether the buffer at `path` (absolute, or empty for a scratch buffer)
/// in `language` must not be sent. `home` expands `~/` in `extra`.
pub fn excluded(path: &str, language: &str, extra: &[String], home: Option<&str>) -> bool {
   if LANGUAGES.contains(&language) {
      return true;
   }
   let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
      return false;
   };
   let matches = |pattern: &str| match pattern.strip_prefix("~/") {
      Some(rest) => {
         home.is_some_and(|home| glob(&format!("{}/{rest}", home.trim_end_matches('/')), path))
      }
      None if pattern.contains('/') => glob(pattern, path),
      None => glob(pattern, name),
   };
   NAMES.iter().copied().any(matches) || extra.iter().any(|pattern| matches(pattern))
}

/// Whether `text` matches `pattern` as a whole, `*` standing for any run of
/// characters and `?` for one.
fn glob(pattern: &str, text: &str) -> bool {
   let pattern: Vec<char> = pattern.chars().collect();
   let text: Vec<char> = text.chars().collect();
   // The classic two-pointer match, going back to the last star on a miss.
   let (mut p, mut t) = (0, 0);
   let mut star: Option<(usize, usize)> = None;
   while t < text.len() {
      match pattern.get(p) {
         Some('*') => {
            star = Some((p, t));
            p += 1;
         }
         Some(&c) if c == '?' || c == text[t] => {
            p += 1;
            t += 1;
         }
         _ => match star {
            Some((sp, st)) => {
               p = sp + 1;
               t = st + 1;
               star = Some((sp, st + 1));
            }
            None => return false,
         },
      }
   }
   pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn globs() {
      assert!(glob("*.pem", "server.pem"));
      assert!(!glob("*.pem", "server.pem.rs"));
      assert!(glob(".env.*", ".env.local"));
      assert!(!glob(".env.*", ".env"));
      assert!(glob("id_rsa*", "id_rsa.pub"));
      assert!(glob("a?c", "abc"));
      assert!(glob("*a*b*", "xxaxxbxx"));
      assert!(!glob("*a*b", "xxaxxbxx"));
      assert!(glob("*", ""));
   }

   #[test]
   fn secrets_are_kept_out() {
      let none: &[String] = &[];
      for path in [
         "/p/.env",
         "/p/.env.production",
         "/h/.ssh/id_ed25519",
         "/h/.netrc",
         "/h/.aws/credentials",
         "/p/tls/server.key",
      ] {
         assert!(excluded(path, "", none, None), "{path}");
      }
      assert!(excluded("/p/whatever", "env", none, None));
      for path in [
         "/p/src/main.rs",
         "/p/src/env.rs",
         "/p/identity.py",
         "",
         "/p/keys.rs",
      ] {
         assert!(!excluded(path, "rust", none, None), "{path}");
      }
   }

   #[test]
   fn the_setting_adds_patterns() {
      let extra = [
         "*.secret".to_string(),
         "~/notes/*".to_string(),
         "/srv/*/conf.toml".to_string(),
      ];
      let home = Some("/h");
      assert!(excluded("/p/a.secret", "", &extra, home));
      assert!(excluded("/h/notes/diary.md", "markdown", &extra, home));
      assert!(!excluded("/h/code/diary.md", "markdown", &extra, home));
      assert!(excluded("/srv/app/conf.toml", "toml", &extra, home));
      assert!(!excluded("/h/notes/diary.md", "markdown", &extra, None));
   }
}
