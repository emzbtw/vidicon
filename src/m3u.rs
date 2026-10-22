//! M3U playlists (M3U ticket 02, SPEC 6.4): read line by line, an entry
//! at a time, so the list is never whole in memory (ADR 0003).
//!
//! An entry runs from `#EXTINF` to the next non-empty line that is not a
//! `#` line, its URL. Other `#` lines and blank lines in between are
//! skipped; a URL with no `#EXTINF` is kept, named after its last path
//! segment; an `#EXTINF` with no URL is skipped and counted. Stream
//! headers come from `#EXTVLCOPT` lines before the URL and a Kodi-style
//! `|` suffix on it (M3U ticket 03).

use std::io::{BufRead, BufReader, Read};

/// One playlist entry, its attributes trimmed, empty ones `None`.
#[derive(Debug, Default, PartialEq)]
pub struct Entry {
    pub title: String,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    /// `tvg-logo`, `tvg-icon` or `logo`.
    pub logo: Option<String>,
    /// `group-title` or `group`, its first value when `;`-separated.
    pub group: Option<String>,
    /// `tvg-chno` or `tvg-ch`, when a number.
    pub chno: Option<i64>,
    /// Without its `|` suffix.
    pub url: String,
    pub headers: Headers,
}

/// The HTTP headers a stream asks for (M3U ticket 03). Values never hold
/// a control character, so none can end a header early.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers {
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    pub origin: Option<String>,
}

impl Headers {
    /// Sets `key`'s header, `#EXTVLCOPT` (`http-user-agent`) or pipe
    /// suffix (`User-Agent`) style, unless set already. Other keys and
    /// empty values are ignored.
    fn set(&mut self, key: &str, value: &str) {
        let slot = match key.trim().to_ascii_lowercase().as_str() {
            "http-user-agent" | "user-agent" => &mut self.user_agent,
            "http-referrer" | "referer" => &mut self.referrer,
            "http-origin" | "origin" => &mut self.origin,
            _ => return,
        };
        if value.chars().any(char::is_control) {
            return;
        }
        let value = value.trim();
        if !value.is_empty() {
            slot.get_or_insert_with(|| value.to_owned());
        }
    }
}

/// `url` without its Kodi-style `|User-Agent=...&Referer=...` suffix, the
/// suffix's percent-encoded values added to `headers`.
fn split_url(url: &str, headers: &mut Headers) -> String {
    let Some((url, suffix)) = url.split_once('|') else {
        return url.to_owned();
    };
    for pair in suffix.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            headers.set(key, &percent_decode(value));
        }
    }
    url.trim_end().to_owned()
}

/// `%XX` escapes decoded; a `%` not followed by two hex digits is kept.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (b[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Why reading stopped: `each` failed, or the body is not a channel list.
pub enum ReadError<E> {
    Each(E),
    /// Already a message, quoting nothing from the body.
    Body(String),
}

const NOT_M3U: &str = "not an M3U playlist: it does not start with #EXTM3U";

/// Reads `body`, handing each entry to `each` in file order. Returns how
/// many entries were skipped for having no URL. Refuses a body that does
/// not start with `#EXTM3U` (after a UTF-8 BOM and blank lines) and an
/// HLS stream playlist (`#EXT-X-TARGETDURATION` or `#EXT-X-STREAM-INF`).
pub fn read<E>(
    body: impl Read,
    mut each: impl FnMut(Entry) -> Result<(), E>,
) -> Result<usize, ReadError<E>> {
    let mut body = BufReader::with_capacity(64 * 1024, body);
    let (mut line, mut first, mut header) = (Vec::new(), true, false);
    let mut pending: Option<Entry> = None;
    // `#EXTVLCOPT` headers since the last `#EXTINF` or URL.
    let mut headers = Headers::default();
    let mut skipped = 0;
    loop {
        line.clear();
        let n = body
            .read_until(b'\n', &mut line)
            .map_err(|e| ReadError::Body(format!("playlist download interrupted: {e}")))?;
        if n == 0 {
            break;
        }
        let mut raw = &line[..];
        if std::mem::replace(&mut first, false) {
            raw = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(raw);
        }
        let text = String::from_utf8_lossy(raw);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if !std::mem::replace(&mut header, true) && strip_ci(text, "#EXTM3U").is_none() {
            return Err(ReadError::Body(NOT_M3U.into()));
        }
        if let Some(rest) = strip_ci(text, "#EXTINF:") {
            skipped += usize::from(pending.is_some());
            pending = Some(extinf(rest));
            headers = Headers::default();
        } else if let Some(opt) = strip_ci(text, "#EXTVLCOPT:") {
            if let Some((key, value)) = opt.split_once('=') {
                headers.set(key, value);
            }
        } else if strip_ci(text, "#EXT-X-TARGETDURATION").is_some()
            || strip_ci(text, "#EXT-X-STREAM-INF").is_some()
        {
            return Err(ReadError::Body(
                "this is a stream, not a channel list".into(),
            ));
        } else if !text.starts_with('#') {
            let mut headers = std::mem::take(&mut headers);
            let url = split_url(text, &mut headers);
            let entry = match pending.take() {
                Some(e) => Entry { url, headers, ..e },
                None => Entry {
                    title: last_segment(&url).to_owned(),
                    url,
                    headers,
                    ..Entry::default()
                },
            };
            each(entry).map_err(ReadError::Each)?;
        }
    }
    if !header {
        return Err(ReadError::Body(NOT_M3U.into()));
    }
    Ok(skipped + usize::from(pending.is_some()))
}

/// An `#EXTINF` line's attributes and title, waiting for its URL.
fn extinf(rest: &str) -> Entry {
    // The duration comes first, up to whitespace or the title's comma.
    let start = rest
        .find(|c: char| c.is_whitespace() || c == ',')
        .unwrap_or(rest.len());
    let (attrs, comma) = attributes(&rest[start..]);
    let mut e = Entry {
        title: comma
            .map_or("", |(i, _)| rest[start + i + 1..].trim())
            .to_owned(),
        ..Entry::default()
    };
    for a in attrs {
        let v = a.value.trim();
        if v.is_empty() {
            continue;
        }
        let slot = match a.key.as_str() {
            "tvg-id" => &mut e.tvg_id,
            "tvg-name" => &mut e.tvg_name,
            "tvg-logo" | "tvg-icon" | "logo" => &mut e.logo,
            "group-title" | "group" => &mut e.group,
            "tvg-chno" | "tvg-ch" => {
                e.chno = e.chno.or(v.parse().ok());
                continue;
            }
            _ => continue,
        };
        slot.get_or_insert_with(|| v.to_owned());
    }
    e.group = e
        .group
        .and_then(|g| Some(g.split(';').next()?.trim().to_owned()))
        .filter(|g| !g.is_empty());
    e
}

/// `prefix` at the start of `s`, ignoring ASCII case; the rest.
pub(crate) fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// The last path segment of a URL or path, without the query.
pub(crate) fn last_segment(url: &str) -> &str {
    let path = url.split(['?', '#', '|']).next().unwrap_or(url);
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// How an attribute value was written.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Quote {
    Double,
    Single,
    None,
}

// `--probe-m3u` alone reads how a value was quoted.
#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct Attr {
    pub key: String,
    pub value: String,
    pub quote: Quote,
    pub terminated: bool,
}

/// `key=value` pairs, values double-quoted, single-quoted or unquoted, up to
/// the first comma outside quotes; and that comma's byte offset with the
/// count of commas outside quotes.
pub(crate) fn attributes(s: &str) -> (Vec<Attr>, Option<(usize, usize)>) {
    // The title starts after the first comma outside quotes (ticket 01: about
    // 330 titles on a provider's list hold commas). Quotes open only right after
    // `=`, so an apostrophe in a title opens nothing.
    let (mut quote, mut first, mut commas, mut prev) = (None::<u8>, None, 0, 0u8);
    for (i, b) in s.bytes().enumerate() {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') if prev == b'=' => quote = Some(b),
            (None, b',') => (first, commas) = (first.or(Some(i)), commas + 1),
            _ => {}
        }
        prev = b;
    }
    let head = first.map_or(s, |i| &s[..i]);
    let mut attrs = Vec::new();
    let mut rest = head;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = rest[..end].to_ascii_lowercase();
        rest = &rest[end..];
        let Some(after) = rest.strip_prefix('=') else {
            continue;
        };
        let (value, quote, terminated, left) = match after.as_bytes().first() {
            Some(&q @ (b'"' | b'\'')) => match after[1..].find(q as char) {
                Some(j) => (&after[1..1 + j], q, true, &after[2 + j..]),
                None => (&after[1..], q, false, ""),
            },
            _ => {
                let j = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..j], 0, true, &after[j..])
            }
        };
        attrs.push(Attr {
            key,
            value: value.to_owned(),
            quote: match quote {
                b'"' => Quote::Double,
                b'\'' => Quote::Single,
                _ => Quote::None,
            },
            terminated,
        });
        rest = left;
    }
    (attrs, first.map(|i| (i, commas)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(body: &str) -> Result<(Vec<Entry>, usize), String> {
        let mut out = Vec::new();
        let skipped = read(body.as_bytes(), |e| {
            out.push(e);
            Ok::<_, ()>(())
        })
        .map_err(|e| match e {
            ReadError::Body(e) => e,
            ReadError::Each(()) => unreachable!(),
        })?;
        Ok((out, skipped))
    }

    #[test]
    fn attributes_read_every_quoting() {
        let (a, comma) = attributes(r#" tvg-id="a,b" tvg-name='Its' group=News x,Bob's, Show"#);
        assert_eq!(a.len(), 3);
        assert_eq!((a[0].key.as_str(), a[0].value.as_str()), ("tvg-id", "a,b"));
        assert_eq!(a[1].quote, Quote::Single);
        assert_eq!((a[2].value.as_str(), a[2].quote), ("News", Quote::None));
        // Two commas outside quotes: the title is after the first one.
        let (i, n) = comma.unwrap();
        assert_eq!(n, 2);
        assert!(
            r#" tvg-id="a,b" tvg-name='Its' group=News x,Bob's, Show"#[i..]
                .starts_with(",Bob's, Show")
        );
        let (a, _) = attributes(r#" tvg-id="open"#);
        assert!(!a[0].terminated);
    }

    #[test]
    fn each_tolerated_form_parses() {
        let body = concat!(
            "\u{feff}#EXTM3U url-tvg=\"http://g.example/guide.xml\"\r\n",
            "#EXTINF:-1 tvg-id=\"harbour.uk\" tvg-name=\"Harbour One\" tvg-logo=\"http://l/1.png\" group-title=\"News;UK\",Harbour One, London\r\n",
            "\r\n",
            "# a comment\r\n",
            "#EXTVLCOPT:http-user-agent=Example/1.0\r\n",
            "http://s.example/1.ts\r\n",
            "#EXTINF:0 tvg-id=kestrel.uk group-title='Drama' tvg-chno=\"12\",Kestrel's Own\n",
            "http://s.example/2.ts|User-Agent=Example\n",
            "#EXT-X-CREDENTIALS:none\n",
            "http://s.example/live/juniper.m3u8?token=t\n",
            "#EXTINF:-1 tvg-ch=x logo=\"http://l/3.png\" group=\"\",\n",
            "rtmp://s.example/3\n",
        );
        let (got, skipped) = entries(body).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(
            got,
            [
                Entry {
                    title: "Harbour One, London".into(),
                    tvg_id: Some("harbour.uk".into()),
                    tvg_name: Some("Harbour One".into()),
                    logo: Some("http://l/1.png".into()),
                    group: Some("News".into()),
                    chno: None,
                    url: "http://s.example/1.ts".into(),
                    headers: Headers {
                        user_agent: Some("Example/1.0".into()),
                        ..Headers::default()
                    },
                },
                Entry {
                    title: "Kestrel's Own".into(),
                    tvg_id: Some("kestrel.uk".into()),
                    group: Some("Drama".into()),
                    chno: Some(12),
                    url: "http://s.example/2.ts".into(),
                    headers: Headers {
                        user_agent: Some("Example".into()),
                        ..Headers::default()
                    },
                    ..Entry::default()
                },
                Entry {
                    title: "juniper.m3u8".into(),
                    url: "http://s.example/live/juniper.m3u8?token=t".into(),
                    ..Entry::default()
                },
                Entry {
                    logo: Some("http://l/3.png".into()),
                    url: "rtmp://s.example/3".into(),
                    ..Entry::default()
                },
            ]
        );
    }

    #[test]
    fn headers_come_from_options_and_the_pipe_suffix() {
        let body = concat!(
            "#EXTM3U\n",
            "#EXTVLCOPT:http-user-agent=Opt/1.0 (X11, Linux)\n",
            "#EXTINF:-1,One\n",
            "#EXTVLCOPT:http-referrer=https://r.example/\n",
            "#EXTVLCOPT:HTTP-ORIGIN=https://o.example\n",
            "#EXTVLCOPT:network-caching=1000\n",
            "#KODIPROP:inputstream=x\n",
            "http://s/1.m3u8|User-Agent=Pipe%2F2.0&Referer=https://p.example/&Cookie=c=1\n",
            "#EXTINF:-1,Two\n",
            "http://s/2.ts | user-agent=Two%20Agent&origin=https%3A%2F%2Ftwo.example&referer=%0d%0aX: y\n",
            "#EXTINF:-1,Three\n",
            "http://s/3.ts\n",
        );
        let (got, _) = entries(body).unwrap();
        let shown: Vec<(&str, &Headers)> =
            got.iter().map(|e| (e.url.as_str(), &e.headers)).collect();
        let h = |ua: Option<&str>, referrer: Option<&str>, origin: Option<&str>| Headers {
            user_agent: ua.map(Into::into),
            referrer: referrer.map(Into::into),
            origin: origin.map(Into::into),
        };
        assert_eq!(
            shown,
            [
                // An option above the `#EXTINF` is not the entry's; one
                // below wins over the suffix.
                (
                    "http://s/1.m3u8",
                    &h(
                        Some("Pipe/2.0"),
                        Some("https://r.example/"),
                        Some("https://o.example")
                    )
                ),
                // A decoded CR LF drops the value.
                (
                    "http://s/2.ts",
                    &h(Some("Two Agent"), None, Some("https://two.example"))
                ),
                ("http://s/3.ts", &Headers::default()),
            ]
        );
    }

    #[test]
    fn entries_without_a_url_are_skipped_and_counted() {
        let body = "#EXTM3U\n#EXTINF:-1,One\n#EXTINF:-1,Two\nhttp://s/2\n#EXTINF:-1,Three\n";
        let (got, skipped) = entries(body).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Two");
        assert_eq!(skipped, 2);
    }

    #[test]
    fn rejects_what_is_not_a_channel_list() {
        for (body, why) in [
            ("", "#EXTM3U"),
            ("\n\n", "#EXTM3U"),
            ("<html>sign in</html>\n", "#EXTM3U"),
            ("#EXTINF:-1,One\nhttp://s/1\n", "#EXTM3U"),
            (
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg1.ts\n",
                "a stream",
            ),
            (
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nlow.m3u8\n",
                "a stream",
            ),
        ] {
            let err = entries(body).unwrap_err();
            assert!(err.contains(why), "{body:?}: {err}");
        }
    }
}
