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

    #[instrument(skip(self, pack), fields(id = %pack.id))]
    pub async fn publish(&self, pack: &Package) -> Result<bool> {
        let ret = self.runtime.cache().store().publish(pack).await?;
        Ok(ret)
    }

    #[instrument(skip(self, q))]
    pub async fn list(&self, q: &Query) -> Result<PageData<PackageInfo>> {
        match self.runtime.cache().store().packages().query(q).await {
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

    #[instrument(skip(self), fields(id = %id))]
    pub async fn get(&self, id: &str) -> Result<PackageInfo> {
        let package = &self.runtime.cache().store().packages().find(id).await?;
        Ok(package.into())
    }

    #[instrument(skip(self), fields(id = %id))]
    pub async fn rm(&self, id: &str) -> Result<bool> {
        self.runtime.cache().store().packages().delete(id).await
    }
}
