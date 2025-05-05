use crate::{ActError, Result, Step};
use regex::Regex;
use serde::{Deserialize, Serialize, de};
use std::str::FromStr;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Timeout {
    #[serde(default)]
    pub on: String,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize, Clone)]
pub enum TimeoutUnit {
    #[default]
    Second,
    Minute,
    Hour,
    Day,
}

#[derive(Debug, Default, PartialEq, Clone)]
pub struct TimeoutLimit {
    pub value: i64,
    pub unit: TimeoutUnit,
}

impl FromStr for TimeoutLimit {
    type Err = ActError;
    fn from_str(s: &str) -> Result<Self> {
        let value = TimeoutLimit::parse(s)?;
        Ok(value)
    }
}

impl std::fmt::Display for TimeoutLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{}{}",
            self.value,
            match self.unit {
                TimeoutUnit::Second => "s",
                TimeoutUnit::Minute => "m",
                TimeoutUnit::Hour => "h",
                TimeoutUnit::Day => "d",
            }
        ))
    }
}

impl<'de> Deserialize<'de> for TimeoutLimit {
    fn deserialize<D>(deserializer: D) -> std::prelude::v1::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TimeoutVisitor;
        impl de::Visitor<'_> for TimeoutVisitor {
            type Value = TimeoutLimit;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("step timeout")
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                TimeoutLimit::parse(v).map_err(|err| E::custom(err))
            }
        }
        deserializer.deserialize_any(TimeoutVisitor)
    }
}

impl Serialize for TimeoutLimit {
    fn serialize<S>(&self, serializer: S) -> std::prelude::v1::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl TimeoutLimit {
    pub fn parse(expr: &str) -> Result<Self> {
        let re = Regex::new(r"^(.*)(s|m|h|d)$").unwrap();
        let caps = re.captures(expr);

        if let Some(caps) = caps {
            let value = caps.get(1).map_or("0", |m| m.as_str());
            let unit = caps.get(2).map_or("s", |m| m.as_str());

            let value = value
                .parse::<i64>()
                .map_err(|err| ActError::Model(format!("timeout parse error with '{err}'")))?;
            let unit = TimeoutUnit::parse(unit)?;

            return Ok(Self { value, unit });
        }

        Err(ActError::Model(format!(
            "timeout parse error with '{expr}'"
        )))
    }

    pub fn as_secs(&self) -> i64 {
        match self.unit {
            TimeoutUnit::Second => self.value,
            TimeoutUnit::Minute => self.value * 60,
            TimeoutUnit::Hour => self.value * 60 * 60,
            TimeoutUnit::Day => self.value * 60 * 60 * 24,
        }
    }
}

impl TimeoutUnit {
    fn parse(expr: &str) -> Result<Self> {
        match expr {
            "s" => Ok(TimeoutUnit::Second),
            "m" => Ok(TimeoutUnit::Minute),
            "h" => Ok(TimeoutUnit::Hour),
            "d" => Ok(TimeoutUnit::Day),
            _ => Err(ActError::Model(format!(
                "timeout parse error with '{expr}'"
            ))),
        }
    }
}

impl Timeout {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_on(mut self, v: &str) -> Self {
        // self.on = TimeoutLimit::parse(v)
        //     .unwrap_or_else(|_| panic!("failed with error format '{v}' for 'on' "));
        self.on = v.to_string();
        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        self.steps.push(build(Step::default()));

        self
    }
}
