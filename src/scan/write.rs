//! Store a scanned tree in the snapshot tables.

use rusqlite::{Connection, params};

use super::tree::{PathOrder, Tree};
use super::{ScanError, ScanOptions};
use crate::error::Result;
use crate::traverse::FileEntry;

fn sql_count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Write the snapshot tables in one transaction and return the number of
/// stored directories.
pub(super) fn write_tables(
    connection: &mut Connection,
    options: &ScanOptions,
    tree: &Tree,
    large_files: Vec<FileEntry>,
    errors: &[ScanError],
) -> Result<u64> {
    let transaction = connection.transaction()?;
    let order = tree.path_order();
    write_directories(&transaction, tree, &order, options.record_max_depth)?;
    if options.collect_users {
        write_owners(&transaction, tree, &order, options.user_max_depth)?;
    }
    drop(order);
    write_large_files(&transaction, &large_files)?;
    write_errors(&transaction, errors)?;
    let stored: i64 =
        transaction.query_row("SELECT COUNT(*) FROM directories", [], |row| row.get(0))?;
    transaction.commit()?;
    Ok(u64::try_from(stored).unwrap_or(0))
}

fn write_directories(
    connection: &Connection,
    tree: &Tree,
    order: &PathOrder,
    max_depth: Option<u32>,
) -> Result<()> {
    // Two names can be stored as the same path, as `\xff` escapes an invalid
    // byte and is also a valid name; their rows are merged.
    let mut statement = connection.prepare(
        "INSERT INTO directories(path, depth, allocated_bytes, apparent_bytes, file_count, \
         directory_count, inode_count, latest_modified_epoch) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(path) DO UPDATE SET \
         allocated_bytes = allocated_bytes + excluded.allocated_bytes, \
         apparent_bytes = apparent_bytes + excluded.apparent_bytes, \
         file_count = file_count + excluded.file_count, \
         directory_count = directory_count + excluded.directory_count, \
         inode_count = inode_count + excluded.inode_count, \
         latest_modified_epoch = max(latest_modified_epoch, excluded.latest_modified_epoch)",
    )?;
    tree.visit_in_path_order(order, max_depth, |directory, path| {
        let totals = directory.totals;
        statement.execute(params![
            path,
            directory.depth,
            sql_count(totals.allocated_bytes),
            sql_count(totals.apparent_bytes),
            sql_count(totals.file_count),
            sql_count(totals.directory_count),
            sql_count(totals.inode_count),
            totals.latest_modified_epoch,
        ])?;
        Ok(())
    })
}

fn write_owners(
    connection: &Connection,
    tree: &Tree,
    order: &PathOrder,
    max_depth: u32,
) -> Result<()> {
    let mut statement = connection.prepare(
        "INSERT INTO owner_directories(uid, path, depth, allocated_bytes, apparent_bytes, \
         file_count, directory_count, inode_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(uid, path) DO UPDATE SET \
         allocated_bytes = allocated_bytes + excluded.allocated_bytes, \
         apparent_bytes = apparent_bytes + excluded.apparent_bytes, \
         file_count = file_count + excluded.file_count, \
         directory_count = directory_count + excluded.directory_count, \
         inode_count = inode_count + excluded.inode_count",
    )?;
    // One walk per owner writes the rows in (uid, path) key order.
    let mut uids: Vec<u32> = tree
        .nodes()
        .iter()
        .filter(|node| node.depth <= max_depth)
        .flat_map(|node| node.owners.iter().map(|(uid, _)| *uid))
        .collect();
    uids.sort_unstable();
    uids.dedup();
    for uid in uids {
        tree.visit_in_path_order(order, Some(max_depth), |directory, path| {
            if let Some(usage) = directory.owners.get(uid) {
                statement.execute(params![
                    uid,
                    path,
                    directory.depth,
                    sql_count(usage.allocated),
                    sql_count(usage.apparent),
                    sql_count(usage.files),
                    sql_count(usage.directories),
                    sql_count(usage.inodes),
                ])?;
            }
            Ok(())
        })?;
    }
    let mut statement = connection.prepare(
        "INSERT INTO owners(uid, allocated_bytes, apparent_bytes, file_count, directory_count, \
         inode_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut owners: Vec<_> = tree.root().owners.iter().collect();
    owners.sort_unstable_by_key(|(uid, _)| *uid);
    for (uid, usage) in owners {
        statement.execute(params![
            uid,
            sql_count(usage.allocated),
            sql_count(usage.apparent),
            sql_count(usage.files),
            sql_count(usage.directories),
            sql_count(usage.inodes),
        ])?;
    }
    Ok(())
}

fn write_large_files(connection: &Connection, large_files: &[FileEntry]) -> Result<()> {
    // Equal stored paths keep the larger file; the list is largest first.
    let mut statement = connection.prepare(
        "INSERT INTO large_files(path, uid, allocated_bytes, apparent_bytes, modified_epoch) \
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(path) DO NOTHING",
    )?;
    for file in large_files {
        statement.execute(params![
            file.path,
            file.uid,
            sql_count(file.allocated),
            sql_count(file.apparent),
            file.modified,
        ])?;
    }
    Ok(())
}

fn write_errors(connection: &Connection, errors: &[ScanError]) -> Result<()> {
    let mut statement = connection.prepare(
        "INSERT INTO scan_errors(path, operation, kind, message) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for error in errors {
        statement.execute(params![
            error.path,
            error.operation,
            error.kind(),
            error.message
        ])?;
    }
    Ok(())
}
