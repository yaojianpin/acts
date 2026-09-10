use crate::{ActError, Result, Vars};
use rquickjs::{
    Array as JsArray, FromJs, Function as JsFunction, IntoAtom, IntoJs, Object as JsObject,
    String as JsString, Value as JsValue,
};
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct ActJsValue(serde_json::Value);

impl ActJsValue {
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

impl<'js> IntoJs<'js> for ActJsValue {
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
                JsValue::from_string(JsString::from_str(ctx.clone(), &v)?)
            }
            serde_json::Value::Array(v) => {
                let arr = JsArray::new(ctx.clone())?;
                for (idx, v) in v.iter().enumerate() {
                    let val = ActJsValue(v.clone()).into_js(ctx)?;
                    arr.set(idx, val)?;
                }
                JsValue::from_array(arr)
            }
            serde_json::Value::Object(v) => {
                let obj = JsObject::new(ctx.clone())?;
                for (k, v) in v {
                    obj.set(k.into_atom(ctx)?, ActJsValue(v).into_js(ctx)?)?;
                }

                JsValue::from_object(obj)
            }
        };

        Ok(value)
    }
}

impl<'js> FromJs<'js> for ActJsValue {
    fn from_js(ctx: &rquickjs::Ctx<'js>, v: JsValue<'js>) -> rquickjs::Result<Self> {
        let result = match v.type_of() {
            rquickjs::Type::Null | rquickjs::Type::Undefined | rquickjs::Type::Uninitialized => {
                Ok(serde_json::json!(null))
            }
            rquickjs::Type::Bool => Ok(serde_json::json!(v.as_bool().unwrap_or(false))),
            rquickjs::Type::Int => Ok(serde_json::json!(v.as_int().unwrap_or(0))),
            rquickjs::Type::Float => Ok(serde_json::json!(v.as_float().unwrap_or(0.0))),
            rquickjs::Type::String => Ok(serde_json::json!(
                v.as_string()
                    .ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            v.type_name(),
                            "string",
                            "expected a JS string",
                        )
                    })?
                    .to_string()?
            )),
            rquickjs::Type::Array => {
                let array = v.as_array().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "Array",
                        "expected a JS array",
                    )
                })?;
                let values = array
                    .iter::<JsValue>()
                    .map(|item| {
                        let item = item?;
                        Ok(ActJsValue::from_js(ctx, item)?.into())
                    })
                    .collect::<rquickjs::Result<Vec<_>>>()?;
                Ok(serde_json::Value::Array(values))
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
                    value.insert(k.clone(), ActJsValue::from_js(ctx, v)?.into());
                }
                Ok(serde_json::Value::Object(value))
            }
            rquickjs::Type::BigInt => {
                let bigint = v.as_big_int().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "BigInt",
                        "expected a BigInt",
                    )
                })?;

                // `JS_ToInt64` wraps oversized BigInts, so derive its canonical
                // decimal representation and range-check it before narrowing.
                let to_string = ctx.globals().get::<_, JsFunction>("String")?;
                let text = to_string
                    .call::<_, JsValue>((bigint.clone(),))?
                    .as_string()
                    .ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            v.type_name(),
                            "string",
                            "cannot stringify a BigInt",
                        )
                    })?
                    .to_string()?;
                let negative = text.starts_with('-');
                let digits = text.trim_start_matches('-');
                let magnitude: i128 = digits.parse().map_err(|_| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "i64",
                        "invalid BigInt value",
                    )
                })?;
                let signed = if negative { -magnitude } else { magnitude };
                let v = i64::try_from(signed).map_err(|_| {
                    rquickjs::Error::new_from_js_message(
                        v.type_name(),
                        "i64",
                        "BigInt value is outside the i64 range",
                    )
                })?;
                Ok(serde_json::json!(v))
            }
            rquickjs::Type::Exception => {
                let ex = v
                    .as_exception()
                    .ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            v.type_name(),
                            "Exception",
                            "expected a JS exception",
                        )
                    })?
                    .clone();
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

        Ok(ActJsValue(result))
    }
}

impl From<ActJsValue> for serde_json::Value {
    fn from(val: ActJsValue) -> Self {
        val.0
    }
}

impl From<Vars> for ActJsValue {
    fn from(value: Vars) -> Self {
        ActJsValue(value.into())
    }
}

impl From<String> for ActJsValue {
    fn from(value: String) -> Self {
        ActJsValue(value.into())
    }
}
