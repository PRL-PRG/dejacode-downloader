use crate::updater::*;
use crate::records::*;

pub (crate) fn task_load_substore(updater : & Updater, store : StoreKind,  task : TaskStatus) -> Result<(), std::io::Error> {
    updater.ds.substore(store).load(& task);
    task.info(format!("{:?}", store));
    return Ok(());
}