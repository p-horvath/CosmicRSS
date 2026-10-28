use std::collections::HashSet;

use quick_xml::events::{BytesDecl, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::feed::Channel;

/// A single feed reference in an OPML document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRef {
    pub title: String,
    pub url: String,
}

/// Serializes channels into an OPML 1.0 document (one `<outline>` per channel).
pub fn export_opml(channels: &[Channel]) -> String {
    let mut buf = Vec::new();
    let mut writer = Writer::new_with_indent(&mut buf, b' ', 2);
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .expect("writing to a Vec cannot fail");
    let _ = writer
        .create_element("opml")
        .with_attribute(("version", "1.0"))
        .write_inner_content(|writer| {
            let _ = writer
                .create_element("head")
                .write_inner_content(|writer| {
                    let _ = writer
                        .create_element("title")
                        .write_text_content(BytesText::new("CosmicRSS feeds"))?;
                    Ok(())
                })?;
            let _ = writer.create_element("body").write_inner_content(|writer| {
                for channel in channels {
                    let _ = writer
                        .create_element("outline")
                        .with_attribute(("text", channel.title.as_str()))
                        .with_attribute(("title", channel.title.as_str()))
                        .with_attribute(("xmlUrl", channel.url.as_str()))
                        .write_empty()?;
                }
                Ok(())
            })?;
            Ok(())
        })
        .expect("writing to a Vec cannot fail");
    String::from_utf8(buf).expect("writer output is UTF-8")
}

/// Parses an OPML document and collects feed references.
///
/// Flattening: any `<outline>` with an `xmlUrl` attribute at any depth is
/// accepted, so nested category nodes exported by other readers import fine.
/// Outlines without `xmlUrl` (group nodes) are ignored, and duplicate URLs
/// are dropped (first occurrence wins). The `title` comes from the `text`
/// attribute, falling back to the URL.
pub fn parse_opml(data: &str) -> anyhow::Result<Vec<FeedRef>> {
    let mut reader = Reader::from_str(data);
    let mut feeds = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let event = reader.read_event()?;
        if let Event::Start(tag) | Event::Empty(tag) = &event {
            if tag.local_name().into_inner() == b"outline" {
                let mut url = None;
                let mut text = None;
                for attr in tag.attributes() {
                    let Ok(attr) = attr else {
                        continue;
                    };
                    if attr.key.0 == b"xmlUrl" {
                        url = Some(decode_value(&attr.value)?);
                    } else if attr.key.0 == b"text" {
                        text = Some(decode_value(&attr.value)?);
                    }
                }
                if let Some(url) = url {
                    if seen.insert(url.clone()) {
                        feeds.push(FeedRef {
                            title: text.unwrap_or_else(|| url.clone()),
                            url,
                        });
                    }
                }
            }
        }
        if matches!(event, Event::Eof) {
            break;
        }
    }
    if feeds.is_empty() {
        return Err(anyhow::anyhow!(
            "no feeds found in OPML document (no outline with an xmlUrl attribute)"
        ));
    }
    Ok(feeds)
}

/// Decodes an attribute value borrowed from the (UTF-8) input document.
fn decode_value(value: &std::borrow::Cow<'_, [u8]>) -> anyhow::Result<String> {
    std::str::from_utf8(value)
        .map(str::to_string)
        .map_err(|_| anyhow::anyhow!("attribute value is not valid UTF-8"))
}
