use crate::state::Track;
use anyhow::Result;
use reqwest::{StatusCode, Url, blocking::Client};
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::Read,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

const MUSICBRAINZ: &str = "https://musicbrainz.org/ws/2/release-group/";
const COVER_ART_ARCHIVE: &str = "https://coverartarchive.org/release-group";
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Album {
    title: String,
    artist: String,
}

impl Album {
    fn from_track(track: &Track) -> Option<Self> {
        let title = track.album.trim();
        let artist = track.artist.trim();
        if title.is_empty() || artist.is_empty() {
            return None;
        }
        Some(Self {
            title: title.to_owned(),
            artist: artist.to_owned(),
        })
    }
}

pub struct Artwork {
    requests: Sender<Option<Album>>,
    results: Receiver<(Album, Option<String>)>,
    requested: Option<Album>,
    cover: Option<String>,
}

impl Artwork {
    pub fn spawn() -> Result<Self> {
        let client = Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(3))
            .user_agent(concat!(
                "t-ui-uxplay/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/kiensony/t-ui-uxplay)"
            ))
            .build()?;
        let (requests, inbox) = mpsc::channel::<Option<Album>>();
        let (reply, results) = mpsc::channel();
        thread::Builder::new()
            .name("discord-artwork".into())
            .spawn(move || {
                let mut cache = HashMap::<Album, Option<String>>::new();
                let mut next_lookup = Instant::now();
                while let Ok(request) = inbox.recv() {
                    thread::sleep(next_lookup.saturating_duration_since(Instant::now()));
                    let Some(album) = inbox.try_iter().last().unwrap_or(request) else {
                        continue;
                    };
                    let cover = if let Some(cached) = cache.get(&album) {
                        cached.clone()
                    } else {
                        next_lookup = Instant::now() + Duration::from_secs(1);
                        match resolve(&client, &album) {
                            Ok(cover) => {
                                cache.insert(album.clone(), cover.clone());
                                cover
                            }
                            Err(_) => None,
                        }
                    };
                    if reply.send((album, cover)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            requested: None,
            cover: None,
        })
    }

    pub fn cover(&mut self, track: Option<&Track>) -> Option<&str> {
        let album = track.and_then(Album::from_track);
        if album != self.requested {
            self.cover = None;
            let _ = self.requests.send(album.clone());
            self.requested = album;
        }
        for (album, cover) in self.results.try_iter() {
            if self.requested.as_ref() == Some(&album) {
                self.cover = cover;
            }
        }
        self.cover.as_deref()
    }
}

fn resolve(client: &Client, album: &Album) -> Result<Option<String>> {
    let query = release_query(album);
    let response = client
        .get(MUSICBRAINZ)
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
        .send()?
        .error_for_status()?;
    let results: Search = serde_json::from_reader(response.take(MAX_RESPONSE_BYTES))?;
    let Some(release) = results.matching_release(album) else {
        return Ok(None);
    };
    let response = client
        .get(format!("{COVER_ART_ARCHIVE}/{}", release.id))
        .send()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let covers: CoverList =
        serde_json::from_reader(response.error_for_status()?.take(MAX_RESPONSE_BYTES))?;
    Ok(covers.front_image())
}

fn release_query(album: &Album) -> String {
    let escape = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"releasegroup:"{}" AND artist:"{}""#,
        escape(&album.title),
        escape(&album.artist)
    )
}

#[derive(Deserialize)]
struct Search {
    #[serde(rename = "release-groups")]
    releases: Vec<Release>,
}

impl Search {
    fn matching_release(&self, album: &Album) -> Option<&Release> {
        let title = album.title.to_lowercase();
        let artist = album.artist.to_lowercase();
        self.releases.iter().find(|release| {
            release.title.trim().to_lowercase() == title
                && release
                    .credits
                    .iter()
                    .flat_map(|credit| [credit.name(), credit.joinphrase.as_str()])
                    .collect::<String>()
                    .trim()
                    .to_lowercase()
                    == artist
        })
    }
}

#[derive(Deserialize)]
struct Release {
    id: String,
    title: String,
    #[serde(rename = "artist-credit", default)]
    credits: Vec<Credit>,
}

#[derive(Deserialize)]
struct Credit {
    name: Option<String>,
    artist: Artist,
    #[serde(default)]
    joinphrase: String,
}

impl Credit {
    fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.artist.name)
    }
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}

#[derive(Deserialize)]
struct CoverList {
    images: Vec<Cover>,
}

impl CoverList {
    fn front_image(&self) -> Option<String> {
        self.images
            .iter()
            .filter(|cover| cover.front)
            .find_map(|cover| {
                cover
                    .thumbnails
                    .medium
                    .iter()
                    .chain(&cover.image)
                    .find_map(|link| {
                        let mut url = Url::parse(link).ok()?;
                        let host = url.host_str()?;
                        if host != "coverartarchive.org"
                            && host != "archive.org"
                            && !host.ends_with(".archive.org")
                        {
                            return None;
                        }
                        url.set_scheme("https").ok()?;
                        Some(url.into())
                    })
            })
    }
}

#[derive(Deserialize)]
struct Cover {
    front: bool,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    thumbnails: Thumbnails,
}

#[derive(Default, Deserialize)]
struct Thumbnails {
    #[serde(rename = "500")]
    medium: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_album_title_and_credited_artist() {
        let results: Search = serde_json::from_str(r#"{
            "release-groups": [
                {"id":"other", "title":"The Album", "artist-credit":[{"artist":{"name":"Someone Else"}}]},
                {"id":"match", "title":"The Album", "artist-credit":[
                    {"name":"First", "artist":{"name":"First Artist"}, "joinphrase":" & "},
                    {"artist":{"name":"Second"}}
                ]}
            ]
        }"#).unwrap();
        let mut album = Album {
            title: "the album".into(),
            artist: "First & Second".into(),
        };
        assert_eq!(results.matching_release(&album).unwrap().id, "match");
        album.artist = "First".into();
        assert!(results.matching_release(&album).is_none());

        album.title = r#"Back\slash "and quotes""#.into();
        assert_eq!(
            release_query(&album),
            r#"releasegroup:"Back\\slash \"and quotes\"" AND artist:"First""#
        );
    }

    #[test]
    fn uses_an_https_front_thumbnail_or_falls_back() {
        let covers: CoverList = serde_json::from_str(
            r#"{
            "images": [
                {"front": false, "image": "https://archive.org/back.png"},
                {"front": true, "image": "http://coverartarchive.org/front.png", "thumbnails": {
                    "500": "http://archive.org/front-500.jpg"
                }}
            ]
        }"#,
        )
        .unwrap();
        assert_eq!(
            covers.front_image().as_deref(),
            Some("https://archive.org/front-500.jpg")
        );
        let missing: CoverList = serde_json::from_str(
            r#"{
            "images": [
                {"front": false, "image": "https://archive.org/back.png"},
                {"front": true, "image": "http://localhost/private.png"}
            ]
        }"#,
        )
        .unwrap();
        assert!(missing.front_image().is_none());
    }

    #[test]
    fn drops_stale_artwork_after_a_track_change() {
        let (requests, inbox) = mpsc::channel();
        let (reply, results) = mpsc::channel();
        let mut artwork = Artwork {
            requests,
            results,
            requested: None,
            cover: None,
        };
        let one = Track {
            album: "One".into(),
            artist: "Artist".into(),
            ..Track::default()
        };
        let two = Track {
            album: "Two".into(),
            ..one.clone()
        };
        assert!(artwork.cover(Some(&one)).is_none());
        let old = inbox.try_recv().unwrap().unwrap();
        reply
            .send((old, Some("https://archive.org/one.jpg".into())))
            .unwrap();
        assert!(artwork.cover(Some(&two)).is_none());
        let current = inbox.try_recv().unwrap().unwrap();
        reply
            .send((current, Some("https://archive.org/two.jpg".into())))
            .unwrap();
        assert_eq!(
            artwork.cover(Some(&two)),
            Some("https://archive.org/two.jpg")
        );
        assert!(artwork.cover(None).is_none());
        assert!(inbox.try_recv().unwrap().is_none());
    }
}
