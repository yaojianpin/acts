use crate::{data::Package, sch::Runtime, store::StoreAdapter, PackageInfo, Query, Result};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct PackageExecutor {
    runtime: Arc<Runtime>,
}

impl PackageExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self))]
    pub fn publish(&self, pack: &Package) -> Result<bool> {
        let ret = self.runtime.cache().store().publish(pack)?;
        Ok(ret)
    }

    #[instrument(skip(self))]
    pub fn list(&self, limit: usize) -> Result<Vec<PackageInfo>> {
        let query = Query::new().set_limit(limit);
        match self.runtime.cache().store().packages().query(&query) {
            Ok(mut packages) => {
                packages.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                let mut ret = Vec::new();
                for t in &packages {
                    ret.push(t.into());
                }

                Ok(ret)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, id: &str) -> Result<PackageInfo> {
        let package = &self.runtime.cache().store().packages().find(id)?;
        Ok(package.into())
    }

    #[instrument(skip(self))]
    pub fn rm(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().packages().delete(id)
    }
}
