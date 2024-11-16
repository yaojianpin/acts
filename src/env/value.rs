use crate::{ActError, Result, Vars};
use rquickjs::{
    Array as JsArray, FromJs, IntoAtom, IntoJs, Object as JsObject, String as JsString,
    Value as JsValue,
};
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct ActValue(serde_json::Value);

impl ActValue {
    pub fn new(v: serde_json::Value) -> Self {
        Self(v)
    }

    pub fn inner(&self) -> &serde_json::Value {
        &self.0
    }

    pub fn to<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value::<T>(self.0.clone()).map_err(|err| ActError::Script(err.to_string()))
    }
}

impl<'js> IntoJs<'js> for ActValue {
    fn into_js(self, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        let value = match self.0 {
            serde_json::Value::Null => JsValue::new_null(ctx.clone()),
            serde_json::Value::Bool(v) => JsValue::new_bool(ctx.clone(), v),
            serde_json::Value::Number(v) => {
                if v.is_i64() {
                    let v = v.as_i64().unwrap_or_default() as i32;
                    JsValue::new_int(ctx.clone(), v)
                } else if v.is_f64() {
                    let v = v.as_f64().unwrap_or_default();
                    JsValue::new_float(ctx.clone(), v)
                } else {
                    let v = v.as_i64().unwrap_or_default() as i32;
                    JsValue::new_int(ctx.clone(), v)
                }
            }
            serde_json::Value::String(v) => {
                JsValue::from_string(JsString::from_str(ctx.clone(), &v).unwrap())
            }
            serde_json::Value::Array(v) => {
                let arr = JsArray::new(ctx.clone()).unwrap();
                for (idx, v) in v.iter().enumerate() {
                    let val = ActValue(v.clone()).into_js(ctx).unwrap();
                    arr.set(idx, val).unwrap();
                }
                JsValue::from_array(arr)
            }
            serde_json::Value::Object(v) => {
                let obj = JsObject::new(ctx.clone()).unwrap();
                for (k, v) in v {
                    obj.set(k.into_atom(ctx).unwrap(), ActValue(v).into_js(ctx).unwrap())
                        .unwrap();
                }

                JsValue::from_object(obj)
            }
        };

        Ok(value)
    }
}

impl<'js> FromJs<'js> for ActValue {
    fn from_js(ctx: &rquickjs::Ctx<'js>, v: JsValue<'js>) -> rquickjs::Result<Self> {
        let result = match v.type_of() {
            rquickjs::Type::Null | rquickjs::Type::Undefined | rquickjs::Type::Uninitialized => {
                Ok(serde_json::json!(null))
            }
            rquickjs::Type::Bool => Ok(serde_json::json!(v.as_bool().unwrap_or(false))),
            rquickjs::Type::Int => Ok(serde_json::json!(v.as_int().unwrap_or(0))),
            rquickjs::Type::Float => Ok(serde_json::json!(v.as_float().unwrap_or(0.0))),
            rquickjs::Type::String => Ok(serde_json::json!(v
                .as_string()
                .unwrap()
                .to_string()
                .unwrap_or(String::from("")))),
            rquickjs::Type::Array => {
                let empty = &JsArray::new(ctx.clone())?;
                Ok(serde_json::Value::Array(
                    v.as_array()
                        .unwrap_or(empty)
                        .iter::<JsValue>()
                        .filter_map(|v| {
                            v.map(|v| ActValue::from_js(ctx, v.clone()).unwrap().into())
                                .ok()
                        })
                        .collect(),
                ))
            }
            rquickjs::Type::Object => {
                let mut value = serde_json::Map::<String, serde_json::Value>::new();
                let inner = JsObject::new(ctx.clone())?;
                let object = v.as_object().unwrap_or(&inner);
                let keys = object
                    .keys::<String>()
                    .filter_map(|v| v.ok())
                    .collect::<Vec<_>>();
                let values = keys
                    .iter()
                    .filter_map(|key| {
                        match object.get::<String, JsValue>(key.clone()) {
                            Ok(value) => Ok((key, value)),
                            Err(err) => Err(err),
                        }
                        .ok()
                    })
                    .collect::<Vec<_>>();
                for (k, v) in values {
                    value.insert(k.clone(), ActValue::from_js(ctx, v)?.into());
                }
                Ok(serde_json::Value::Object(value))
            }
            rquickjs::Type::BigInt => {
                let bigint = v.as_big_int().unwrap().clone();
                let v = bigint.to_i64().unwrap();
                Ok(serde_json::json!(v))
            }
            rquickjs::Type::Exception => {
                let ex = v.as_exception().unwrap().clone();
                Err(ex.throw())
            }
            rquickjs::Type::Unknown
            | rquickjs::Type::Module
            | rquickjs::Type::Constructor
            | rquickjs::Type::Symbol
            | rquickjs::Type::Function
            | rquickjs::Type::Promise => Err(rquickjs::Error::new_from_js_message(
                v.type_name(),
                "",
                "cannot convert js to json value",
            )),
        }?;

        Ok(ActValue(result))
    }
}

impl From<ActValue> for serde_json::Value {
    fn from(val: ActValue) -> Self {
        val.0
    }
}

impl From<Vars> for ActValue {
    fn from(value: Vars) -> Self {
        ActValue(value.into())
    }
}

impl From<String> for ActValue {
    fn from(value: String) -> Self {
        ActValue(value.into())
    }
}
