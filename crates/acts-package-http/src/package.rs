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
            id: "acts.core.http",
            name: "Http",
            desc: "do a http request",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M53.312 512a458.688 458.688 0 1 1 917.376 0A458.688 458.688 0 0 1 53.312 512z m339.52-376.32a394.048 394.048 0 0 0-138.88 77.632c25.6 22.08 53.952 40.96 84.48 55.936 6.784-25.408 14.528-49.152 23.168-70.848 9.216-22.912 19.584-44.16 31.232-62.72zM209.024 258.944A392.896 392.896 0 0 0 118.592 480h191.232c1.472-51.584 6.528-100.992 14.656-146.688a459.136 459.136 0 0 1-115.456-74.24z m422.144 629.376a394.176 394.176 0 0 0 138.88-77.696 395.328 395.328 0 0 0-84.48-55.936 610.752 610.752 0 0 1-23.168 70.848c-9.152 22.912-19.584 44.096-31.232 62.72z m183.808-123.456A392.832 392.832 0 0 0 905.408 544H714.24a1008.704 1008.704 0 0 1-14.72 146.624c42.24 19.008 81.152 44.16 115.456 74.304zM905.408 480a392.96 392.96 0 0 0-90.432-220.928 459.2 459.2 0 0 1-115.392 74.24c8.064 45.696 13.12 95.104 14.656 146.688h191.168zM769.92 213.312a394.112 394.112 0 0 0-138.88-77.696c11.712 18.688 22.144 39.872 31.296 62.784 8.704 21.76 16.448 45.44 23.104 70.848a395.328 395.328 0 0 0 84.48-55.936zM392.832 888.384a399.04 399.04 0 0 1-31.232-62.784 610.624 610.624 0 0 1-23.104-70.848 395.264 395.264 0 0 0-84.48 55.936 394.048 394.048 0 0 0 138.88 77.696z m-183.744-123.456a459.136 459.136 0 0 1 115.392-74.24A1008.448 1008.448 0 0 1 309.76 544H118.656a392.896 392.896 0 0 0 90.496 220.928zM512 117.312c-11.904 0-26.496 5.952-43.2 23.552-16.64 17.664-33.216 44.928-47.744 81.28-8.448 21.12-16 44.8-22.528 70.656A394.752 394.752 0 0 0 512 309.312c39.488 0 77.568-5.76 113.536-16.512a557.824 557.824 0 0 0-22.528-70.592c-14.592-36.416-31.104-63.68-47.808-81.344-16.64-17.6-31.232-23.552-43.2-23.552zM373.824 480h276.352a953.28 953.28 0 0 0-11.712-124.352A458.88 458.88 0 0 1 512 373.312a458.88 458.88 0 0 1-126.4-17.664A953.216 953.216 0 0 0 373.76 480z m11.776 188.352A458.944 458.944 0 0 1 512 650.688c43.84 0 86.272 6.144 126.464 17.664 6.272-38.656 10.368-80.448 11.712-124.352H373.824c1.344 43.904 5.44 85.76 11.776 124.352zM512 714.688c-39.424 0-77.568 5.76-113.472 16.512 6.464 25.792 14.08 49.472 22.528 70.592 14.528 36.416 31.04 63.68 47.744 81.344 16.64 17.6 31.296 23.552 43.2 23.552 11.968 0 26.56-5.952 43.2-23.552 16.704-17.664 33.28-44.928 47.808-81.28 8.448-21.184 16-44.8 22.464-70.656A394.752 394.752 0 0 0 512 714.688z" ></path></svg>"#,
            doc: "",
            in_schema: include_json!("./in-schema.json"),
            ui_schema: Some(include_json!("./ui-schema.json")),
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

    pub fn run(&self) -> Result<Vars> {
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

        let c = reqwest::blocking::Client::new();
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
            .map_err(|err| ActError::Runtime(format!("Http error: {err}")))?;

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
                    res.text().map_err(map_package_err)?.into(),
                );
            }
            ContentType::Json => {
                ret.insert(
                    DATA_KEY.to_string(),
                    res.json::<serde_json::Value>().map_err(map_package_err)?,
                );
            }
            ContentType::Binary | ContentType::Image | ContentType::Video | ContentType::Audio => {
                let data = res.bytes().map_err(map_package_err)?.to_vec();
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
