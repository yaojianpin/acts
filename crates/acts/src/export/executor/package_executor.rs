use crate::{
    PackageInfo, Result, data::Package, query::Query, scheduler::Runtime, store::PageData,
};
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
    pub fn list(&self, q: &Query) -> Result<PageData<PackageInfo>> {
        match self.runtime.cache().store().packages().query(q) {
            Ok(packages) => Ok(PageData {
                count: packages.count,
                page_size: packages.page_size,
                page_count: packages.page_count,
                page_num: packages.page_num,
                rows: packages.rows.iter().map(|m| m.into()).collect(),
            }),
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
