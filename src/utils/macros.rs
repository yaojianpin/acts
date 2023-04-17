// #[macro_export]
// macro_rules! debug {
//     ($($arg:tt)*) => {
//       #[cfg(feature = "debug")]
//       tracing::debug!($($arg)*);
//     };
// }

// #[macro_export]
// macro_rules! info {
//     ($($arg:tt)*) => {
//       #[cfg(feature = "debug")]
//       tracing::info!($($arg)*);
//     };
// }

// #[macro_export]
// macro_rules! trace {
//   ($($arg:tt)*) => {
//     #[cfg(feature = "trace")]
//     tracing::trace!($($arg)*);
//   };
// }

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::*;
use syn::*;

#[proc_macro]
pub fn db_set(ident: TokenStream) -> TokenStream {
    let db_set_name = format!("{}Collection", ident.to_string());
    let db_set = Ident::new(&db_set_name, Span::call_site()); // 创建新的ident，函数名

    let expanded = quote! {

        #[derive(Debug, Clone)]
        pub struct #db_set {
            name: String,
        }

        // impl DataSet<#ident> for #db_set {
        //     fn exists(&self, id: &str) -> bool {
        //         trace!("local::Model.exists({})", id);
        //         let db = db();
        //         let cf = db.cf_handle(&self.name).unwrap();
        //         match db.get_cf(cf, mode_key(&self.name, id)) {
        //             Ok(opt) => match opt {
        //                 Some(_) => true,
        //                 None => false,
        //             },
        //             Err(_) => false,
        //         }
        //     }

        //     fn find(&self, id: &str) -> ActResult<#ident> {
        //         trace!("local::Model.find({})", id);
        //         let db = db();
        //         let cf = db.cf_handle(&self.name).unwrap();
        //         match db.get_cf(cf, mode_key(&self.name, id)) {
        //             Ok(opt) => match opt {
        //                 Some(data) => {
        //                     let model: $name = bincode::deserialize(data.as_ref()).unwrap();
        //                     Ok(model)
        //                 }
        //                 None => Err(ActError::StoreError(format!("cannot find model id={id}"))),
        //             },
        //             Err(err) => Err(ActError::StoreError(err.to_string())),
        //         }
        //     }

        //     fn query(&self, q: &Query) -> ActResult<Vec<#ident>> {
        //         trace!("local::Model.query({:?})", q);
        //         let mut ret = Vec::new();
        //         let mut limit = q.limit();
        //         if limit == 0 {
        //             // should be a big number to take
        //             limit = 10000;
        //         }
        //         if q.is_cond() {
        //             for id in find_by_idx(&self.name, q) {
        //                 if let Ok(it) = self.find(&id) {
        //                     ret.push(it);
        //                 }
        //             }
        //         } else {
        //             ret = get_all(&self.name, limit);
        //         }

        //         Ok(ret)
        //     }

        //     fn create(&self, model: &#ident) -> ActResult<bool> {
        //         trace!("local::Model.create({})", model.id);
        //         let data = bincode::serialize(model).unwrap();
        //         let db = db();
        //         let cf = db.cf_handle(&self.name).unwrap();
        //         match db.put_cf(cf, mode_key(&self.name, &model.id), data) {
        //             Ok(_) => {
        //                 // let idx = idx_key("id", &proc.id);
        //                 // let value = &format!("{},", proc.id);
        //                 // db.merge_cf(cf, &idx, value).unwrap();
        //                 Ok(true)
        //             }
        //             Err(err) => Err(ActError::StoreError(err.to_string())),
        //         }
        //     }
        //     fn update(&self, model: &#ident) -> ActResult<bool> {
        //         trace!("local::Model.update({})", model.id);
        //         let data = bincode::serialize(model).unwrap();
        //         let db = db();
        //         let cf = db.cf_handle(&self.name).unwrap();
        //         match db.put_cf(cf, mode_key(&self.name, &model.id), data) {
        //             Ok(_) => Ok(true),
        //             Err(err) => Err(ActError::StoreError(err.to_string())),
        //         }
        //     }
        //     fn delete(&self, id: &str) -> ActResult<bool> {
        //         trace!("local::Model.delete({})", id);
        //         let db = db();
        //         let cf = db.cf_handle(&self.name).unwrap();

        //         match self.find(id) {
        //             Ok(item) => match db.delete_cf(cf, mode_key(&self.name, id)) {
        //                 Ok(_) => {
        //                     let idx = idx_key(&self.name, "id", &item.id);
        //                     db.delete_cf(cf, &idx).unwrap();
        //                     Ok(true)
        //                 }
        //                 Err(err) => Err(ActError::StoreError(err.to_string())),
        //             },
        //             Err(err) => Err(ActError::StoreError(err.to_string())),
        //         }
        //     }
        // }
    };
    expanded.into()
}
