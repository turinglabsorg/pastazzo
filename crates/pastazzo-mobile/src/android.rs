use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JObject, JValue};
use jni::sys::jbyteArray;
use pastazzo_sync::secrets::SecretBackend;
use serde_json::{Value, json};
use std::cell::RefCell;
use zeroize::{Zeroize, Zeroizing};

struct AndroidSecrets<'a> {
    env: RefCell<JNIEnv<'a>>,
    object: JObject<'a>,
}

fn refused(env: &mut JNIEnv<'_>) -> String {
    let _ = env.exception_clear();
    "protected key storage refused access".into()
}

impl SecretBackend for AndroidSecrets<'_> {
    fn available(&self) -> bool {
        true
    }

    fn set(&self, user: &str, secret: &[u8]) -> pastazzo_sync::Result<()> {
        let mut env = self.env.borrow_mut();
        let user = env.new_string(user).map_err(|_| refused(&mut env))?;
        let bytes = env
            .byte_array_from_slice(secret)
            .map_err(|_| refused(&mut env))?;
        env.call_method(
            &self.object,
            "set",
            "(Ljava/lang/String;[B)V",
            &[
                JValue::Object(user.as_ref()),
                JValue::Object(bytes.as_ref()),
            ],
        )
        .map_err(|_| refused(&mut env))?;
        Ok(())
    }

    fn get(&self, user: &str) -> pastazzo_sync::Result<Zeroizing<Vec<u8>>> {
        let mut env = self.env.borrow_mut();
        let user = env.new_string(user).map_err(|_| refused(&mut env))?;
        let object = env
            .call_method(
                &self.object,
                "get",
                "(Ljava/lang/String;)[B",
                &[JValue::Object(user.as_ref())],
            )
            .and_then(|value| value.l())
            .map_err(|_| refused(&mut env))?;
        let array = JByteArray::from(object);
        let bytes = Zeroizing::new(
            env.convert_byte_array(&array)
                .map_err(|_| refused(&mut env))?,
        );
        env.set_byte_array_region(&array, 0, &vec![0; bytes.len()])
            .map_err(|_| refused(&mut env))?;
        Ok(bytes)
    }

    fn delete(&self, user: &str) -> pastazzo_sync::Result<()> {
        let mut env = self.env.borrow_mut();
        let user = env.new_string(user).map_err(|_| refused(&mut env))?;
        env.call_method(
            &self.object,
            "delete",
            "(Ljava/lang/String;)V",
            &[JValue::Object(user.as_ref())],
        )
        .map_err(|_| refused(&mut env))?;
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_pastazzo_android_NativeBridge_call<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass<'a>,
    input: JByteArray<'a>,
    secrets: JObject<'a>,
) -> jbyteArray {
    let bytes = match env.convert_byte_array(input) {
        Ok(bytes) if bytes.len() <= 40 * 1024 * 1024 => Zeroizing::new(bytes),
        _ => {
            let _ = env.throw_new(
                "java/lang/IllegalArgumentException",
                "invalid client request",
            );
            return std::ptr::null_mut();
        }
    };
    let backend = AndroidSecrets {
        env: RefCell::new(env),
        object: secrets,
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut request: Value =
            serde_json::from_slice(&bytes).map_err(|_| "invalid JSON request".to_owned())?;
        let result = crate::execute(&request, &backend);
        for name in ["password", "link"] {
            if let Some(Value::String(value)) = request.get_mut(name) {
                value.zeroize();
            }
        }
        result
    }))
    .unwrap_or_else(|_| Err("the mobile client couldn't complete the operation".into()));
    let response = match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    let output = Zeroizing::new(response.to_string().into_bytes());
    backend
        .env
        .borrow_mut()
        .byte_array_from_slice(&output)
        .map(|array| array.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
