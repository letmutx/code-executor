use serde::de::{self, Visitor, Unexpected};
use serde::{Serialize, Serializer, Deserializer, Deserialize};
use std::fmt;

pub struct Tag {
    repository: String,
    tag: String,
}

impl<'a, 'b> From<(&'a str, &'b str)> for Tag {
    fn from(tup: (&'a str, &'b str)) -> Self {
        Tag {
            repository: tup.0.to_owned(),
            tag: tup.1.to_owned(),
        }
    }
}

struct TagVisitor;

impl<'de> Visitor<'de> for TagVisitor {
    type Value = Tag;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "expected string in {{first}}:{{rest}} format")
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        split_at_colon(s).map_err(|_| de::Error::invalid_value(Unexpected::Str(s), &self))
    }
}

impl Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer
    {
        if self.repository.len() == 0 && self.tag.len() == 0 {
            serializer.serialize_str(&"")
        } else {
            let s = self.repository.clone() + &":" + &self.tag;
            serializer.serialize_str(&s)
        }
    }
}

pub struct Id {
    pub encoding: String,
    pub hash: String,
}

impl<'a, 'b> From<(&'a str, &'b str)> for Id {
    fn from(tup: (&'a str, &'b str)) -> Self {
        Id {
            encoding: tup.0.to_owned(),
            hash: tup.1.to_owned(),
        }
    }
}

struct IdVisitor;

impl<'de> Visitor<'de> for IdVisitor {
    type Value = Id;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "expected string in {{first}}:{{rest}} format")
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        split_at_colon(s).map_err(|_| de::Error::invalid_value(Unexpected::Str(s), &self))
    }
}

fn split_at_colon<'a, T>(s: &'a str) -> Result<T, ()>
    where T: From<(&'a str, &'a str)>
{
    let index = s.find(|c| c == ':');
    match index {
        Some(index) => {
            let (first, rest) = s.split_at(index);
            let second = rest.chars().nth(1).unwrap();
            let c = rest.find(second);
            let (_, rest) = rest.split_at(c.unwrap());
            Ok(T::from((first, rest)))
        }
        None => Ok(T::from(("", ""))),
    }
}

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer
    {
        if self.encoding.len() == 0 && self.hash.len() == 0 {
            serializer.serialize_str(&"")
        } else {
            let s = self.encoding.clone() + &":" + &self.hash;
            serializer.serialize_str(&s)
        }
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Id, D::Error>
        where D: Deserializer<'de>
    {
        deserializer.deserialize_str(IdVisitor)
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D>(deserializer: D) -> Result<Tag, D::Error>
        where D: Deserializer<'de>
    {
        deserializer.deserialize_str(TagVisitor)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Port {
    pub IP: Option<String>,
    pub PrivatePort: u64,
    pub PublicPort: Option<u64>,
    pub Type: String,
}