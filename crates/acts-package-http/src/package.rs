use acts::{
    ActError, ActPackage, ActPackageCatalog, ActPackageMeta, ActRunAs, Result, Vars, include_json,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

const DATA_KEY: &str = "data";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub enum ContentType {
    #[serde(rename(deserialize = "none"))]
    None,
    #[serde(rename(deserialize = "text"))]
    Text,
    #[serde(rename(deserialize = "html"))]
    Html,
    #[default]
    #[serde(rename(deserialize = "json"))]
    Json,
    #[serde(rename(deserialize = "urlencoded"))]
    UrlEncoded,
    #[serde(rename(deserialize = "form-data"))]
    FormData,
    #[serde(rename(deserialize = "binary"))]
    Binary,
    #[serde(rename(deserialize = "image"))]
    Image,
    #[serde(rename(deserialize = "video"))]
    Video,
    #[serde(rename(deserialize = "audio"))]
    Audio,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pair {
    pub key: String,
    pub value: JsonValue,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpPackage {
    pub url: String,
    pub method: String,
    #[serde(default)]
    #[serde(rename(deserialize = "content-type"))]
    pub content_type: ContentType,
    #[serde(default)]
    pub headers: Vec<Pair>,
    #[serde(default)]
    pub params: Vec<Pair>,
    pub body: Option<JsonValue>,
}

impl ActPackage for HttpPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.core.http",
            desc: "do a http request",
            version: "0.1.0",
            icon: "icon-http",
            doc: "",
            schema: include_json!("./schema.json"),
            run_as: ActRunAs::Irq,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

impl HttpPackage {
    pub fn create(inputs: &Vars) -> Result<Self> {
        let params = inputs
            .get::<serde_json::Value>("params")
            .ok_or(ActError::Package("missing 'params' in package".to_string()))?;

        let package = serde_json::from_value::<Self>(params)?;
        Ok(package)
    }

    pub async fn run(&self) -> Result<Vars> {
        let mut ret = Vars::new();
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("*/*"),
        );

        for Pair { key, value } in &self.headers {
            headers.insert(
                key.parse::<HeaderName>()
                    .map_err(|err| ActError::Runtime(err.to_string()))?,
                value
                    .to_string()
                    .parse()
                    .map_err(|err: InvalidHeaderValue| ActError::Runtime(err.to_string()))?,
            );
        }
        let mut query = Vec::new();
        for Pair { key, value } in &self.params {
            query.push((key.clone(), value.clone()));
        }

        let c = reqwest::Client::new();
        let mut request = c
            .request(
                self.method
                    .parse()
                    .map_err(|_| ActError::Runtime(format!("invalid method '{}'", self.method)))?,
                &self.url,
            )
            .headers(headers)
            .query(&query);

        match self.content_type {
            ContentType::Text | ContentType::Html => {
                if let Some(text) = &self.body {
                    let data = text.as_str().ok_or(ActError::Package(
                        "content-type did not match the body content".to_string(),
                    ))?;
                    request = request.body::<String>(data.to_string());
                }
            }
            ContentType::Json => {
                if let Some(json) = &self.body {
                    let body = serde_json::to_vec(json)?;
                    request = request.body(body);
                }
            }
            ContentType::FormData | ContentType::UrlEncoded => {
                if let Some(form) = &self.body {
                    let data = form.as_object().ok_or(ActError::Package(
                        "content-type did not match the body content".to_string(),
                    ))?;
                    request = request.form(data);
                }
            }
            ContentType::Binary | ContentType::Image | ContentType::Video | ContentType::Audio => {
                if let Some(value) = &self.body {
                    let data = value.as_str().ok_or(ActError::Package(
                        "content-type did not match the body content".to_string(),
                    ))?;
                    let data = STANDARD
                        .decode(data)
                        .map_err(|err| ActError::Package(err.to_string()))?;
                    request = request.body(data);
                }
            }
            _ => {}
        }

        let res = request
            .send()
            .await
            .map_err(|err| ActError::Runtime(format!("Http error: {}", err)))?;

        let default_type = HeaderValue::from_static("application/json");
        let response_type = res
            .headers()
            .get(CONTENT_TYPE)
            .unwrap_or(&default_type)
            .to_str()
            .map_err(|err| ActError::Package(err.to_string()))?;
        let status = res.status();
        let response_type = get_content_type(response_type);
        match response_type {
            ContentType::Text | ContentType::Html => {
                ret.insert(
                    DATA_KEY.to_string(),
                    res.text().await.map_err(map_package_err)?.into(),
                );
            }
            ContentType::Json => {
                ret.insert(
                    DATA_KEY.to_string(),
                    res.json::<serde_json::Value>()
                        .await
                        .map_err(map_package_err)?,
                );
            }
            ContentType::Binary | ContentType::Image | ContentType::Video | ContentType::Audio => {
                let data = res.bytes().await.map_err(map_package_err)?.to_vec();
                let data = STANDARD.encode(&data);
                ret.insert(DATA_KEY.to_string(), data.into());
            }
            _ => {}
        }
        if !status.is_success() {
            return Err(ActError::Exception {
                ecode: status.as_u16().to_string(),
                message: ret.get(DATA_KEY).unwrap_or(status.to_string()),
            });
        }

        Ok(ret)
    }
}

fn map_package_err(err: reqwest::Error) -> ActError {
    ActError::Package(err.to_string())
}

fn get_content_type(mime_type: &str) -> ContentType {
    let mut ret = ContentType::None;
    if mime_type.starts_with("application/json") {
        ret = ContentType::Json;
    } else if mime_type.starts_with("text/html") {
        ret = ContentType::Html;
    } else if mime_type.starts_with("application/x-www-form-urlencoded") {
        ret = ContentType::UrlEncoded;
    } else if mime_type.starts_with("multipart/form-data") {
        ret = ContentType::FormData;
    } else if mime_type.starts_with("image/") {
        ret = ContentType::Image;
    } else if mime_type.starts_with("audio/") {
        ret = ContentType::Audio;
    } else if mime_type.starts_with("video/") {
        ret = ContentType::Video;
    } else if mime_type.starts_with("text/") || mime_type.starts_with("application/javascript") {
        ret = ContentType::Text;
    }

    ret
}
