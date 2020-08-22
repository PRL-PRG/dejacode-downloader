use std::collections::*;
use serde::{Deserialize, Serialize};
use std::fs::*;
use std::sync::*;
use std::io::prelude::*;
use std::io::SeekFrom;
use std::marker::Sized;
use std::iter::FromIterator;
use db_manager::*;
use byteorder::*;

// TODO how can I make these package only?
// this should be package-only
pub mod db_manager;
pub mod record;
pub mod helpers;


/** Different ids for the entities the database contains.
 */
pub type UserId = u64;
pub type SnapshotId = u64;
pub type BlobId = u64;
pub type PathId = u64;
pub type CommitId = u64;
pub type ProjectId = u64;
pub type Message = Vec<u8>;

/** Source of the information from the downloader. 
 
    For now we only support GHTorrent and GitHub. In the future we might add more. While the downloader exports this, it should not really matter for the users in most cases, other than reliability - stuff coming from GitHub is more reliable than GhTorrent.   
 */
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Source {
    NA,
    Manual,
    GHTorrent,
    GitHub,
}

/** Project is the main gateway to the database. 

    Each project comes with 

    TODO zap last_update
 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    // id of the project
    pub id : ProjectId,
    // url of the project (latest used)
    pub url : String,
    // time at which the project was updated last (i.e. time for which its data are valid)
    pub last_update: i64,
    // metadata information for the project
    pub metadata : HashMap<String,String>,
    // head refs of the project at the last update time
    pub heads : Vec<(String, CommitId)>,
}

/** Single commit information. 
 
    The basic information is required for all commits in the database. Some commits will optionally also return their commit message and changes.
    
    TODO message should I think be bytes, not string because of non-utf-8 garbage. 
 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    // commit id and hash
    pub id : CommitId,  
    pub hash : git2::Oid,
    // id of parents
    pub parents : Vec<CommitId>,
    // committer id and time
    pub committer_id : UserId,
    pub committer_time : i64,
    // author id and time
    pub author_id : UserId,
    pub author_time : i64,
    // commit message
    pub message: Option<Message>,
    // changes made by the commit 
    pub changes: Option<HashMap<PathId, SnapshotId>>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

/** User information. 
 
    Users are unique based on their email.
 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    // id of the user
    pub id : UserId,
    // email for the user
    pub email : String,
    // name of the user
    pub name : String, 
}

/** Snapshot is an unique particular file contents. 
 
    Each snapshot can have metadata and optionally be associated with downloaded contents, which can be retrieved using the blob api.  
 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    // id of the snapshot
    pub id : SnapshotId,
    // contents id, None if the contents was not downloaded
    pub contents : Option<BlobId>,
    // metadata for the snapshot
    pub metadata : HashMap<String, String>,
}

/** Actual contents identified by its  hash. 
    
    TODO the string here should also likely be bytes.
 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    // id of the blob
    pub id : BlobId, 
    // hash of the contents
    pub hash : git2::Oid,
    // the contents
    pub contents : String,
}

/** File path
 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePath {
    // path id
    pub id : PathId,
    // the actual path
    pub path : String,
}

/** A trait for tests. */
pub trait Database {
    fn num_projects(& self) -> u64;
    fn num_commits(& self) -> u64;
    fn num_users(& self) -> u64;
    fn num_file_paths(& self) -> u64;

    fn get_project(& self, id : ProjectId) -> Option<Project>;
    fn get_commit(& self, id : CommitId) -> Option<Commit>;
    fn get_commit_bare(& self, id : CommitId) -> Option<Commit>;
    fn get_user(& self, id : UserId) -> Option<& User>;
    //fn get_snapshot(& self, id : BlobId) -> Option<Snapshot>;
    fn get_file_path(& self, id : PathId) -> Option<FilePath>;

    fn projects(&self) -> Box<dyn Iterator<Item=Project> + '_> where Self: Sized {
        Box::new(ProjectIter::from(self))
    }
    fn commits(&self) -> Box<dyn Iterator<Item=Commit> + '_> where Self: Sized {
        Box::new(CommitIter::from(self))
    }
    fn bare_commits(&self) -> Box<dyn Iterator<Item=Commit> + '_> where Self: Sized {
        Box::new(BareCommitIter::from(self))
    }
    fn users(&self) -> Box<dyn Iterator<Item=&User> + '_> where Self: Sized {
        Box::new(UserIter::from(self))
    }
    fn commits_from(&self, project: &Project) -> Box<dyn Iterator<Item=Commit> + '_> where Self: Sized {
        Box::new(ProjectCommitIter::from(self, project))
    }
    fn bare_commits_from(&self, project: &Project) -> Box<dyn Iterator<Item=Commit> + '_> where Self:Sized {
        Box::new(ProjectBareCommitIter::from(self, project))
    }
    fn user_ids_from(&self, project: &Project) -> Box<dyn Iterator<Item=UserId> + '_> where Self: Sized {
        Box::new(ProjectUserIdIter::from(self, project))
    }
    fn project_ids_and_their_commit_ids(&self) -> Box<dyn Iterator<Item=(ProjectId,Vec<CommitId>)> + '_> where Self: Sized {
        Box::new(ProjectAllCommitIdsIter::from(self))
    }
}

/** The dejacode downloader interface.
 
    Notes on how things are stored (so far only the following things are):

    # Projects

    Each project lives in its own folder and in order to return its representation, notably the log file, metadata file and heads. These must be loaded and analyzed for the project to be constructed. As such projects are not cached and each request will be the disk access. 

    # Commits

    Commit information lives in the following global files:

    - commit_hashes.csv (SHA1 to commit id)
    - commits.csv (time, id, author, committer, author time, committer time, source)
    - commit_parents.csv (time, commit id, parent id)

    Inside the commits and commit parents files each record has a time and newer records *completely* override the information provided in the old records (i.e. first the commit is filled in via ghTorrent, but later when reanalyzed from github directly the information can be updated). The timestamps will allow us to reconstruct the database to any particular time in the past precisely. 

    Commits from the above files are preloaded when DCD is initialized. 
    
    TODO in the future there will also be commit_changes.csv and commit_messages and commit_messages_index.csv for getting changes and messages. 

    # Users

    Similarly to commits, users live in a few global files:

    - user_emails.csv (email to user id)
    - users.csv (time, id, name, source)

    the users file is also timed. 

    TODO in the future perhaps more files, such as metadata. 


 */
pub struct DCD {
    root_ : String,
    num_projects_ : u64,
    users_ : Vec<User>,
    commit_ids_ : HashMap<git2::Oid, CommitId>,
    commit_hashes_ : HashMap<CommitId, git2::Oid>,
    commits_ : Vec<CommitBase>,
    commit_message_offsets_ : HashMap<CommitId, u64>,
    commit_messages_ : Mutex<File>,
    commit_change_offsets_ : HashMap<CommitId, u64>, 
    commit_changes_ : Mutex<File>,

    paths_ : Vec<String>,
}

impl DCD {

    pub fn new(root : String) -> DCD {
        println!("Loading dejacode database...");
        let num_projects = db_manager::DatabaseManager::get_num_projects(& root);
        println!("    {} projects", num_projects);
        let users = Self::get_users(& root);
        println!("    {} users", users.len());
        let commit_ids = db_manager::DatabaseManager::get_commit_ids(& root);
        println!("    {} commits", commit_ids.len());
        let commits = Self::get_commits(& root, commit_ids.len());
        println!("    {} commit records", commit_ids.len());
        let commit_message_offsets = Self::get_commit_message_offsets(& root);
        println!("    {} commit messages", commit_message_offsets.len());
        let commit_messages = OpenOptions::new().read(true).open(DatabaseManager::get_commit_messages_file(& root)).unwrap();
        let commit_change_offsets = Self::get_commit_change_offsets(& root);
        println!("    {} commit changes", commit_change_offsets.len());
        let commit_changes = OpenOptions::new().read(true).open(DatabaseManager::get_commit_changes_file(& root)).unwrap();
        let paths = Self::get_paths(& root);
        println!("    {} paths", paths.len());
        //let snapshots = Self::get_snapshots(& root);
        //println!("    {} snapshots", snapshots.len());
        let commit_hashes : HashMap<CommitId, git2::Oid> = commit_ids.iter().map(|(hash, id)| (*id, *hash)).collect();

        let result = DCD{
            root_ : root, 
            num_projects_ : num_projects,
            users_ : users,
            commit_ids_ : commit_ids,
            commit_hashes_ : commit_hashes,
            commits_ : commits,
            commit_message_offsets_ : commit_message_offsets,
            commit_messages_ : Mutex::new(commit_messages),
            commit_change_offsets_ : commit_change_offsets,
            commit_changes_ : Mutex::new(commit_changes),
            paths_ : paths,
        };
        return result;
    }


    fn get_users(root : & str) -> Vec<User> {
        let mut result = Vec::<User>::new();
        // first load the immutable email to id mapping
        {
            let mut reader = csv::ReaderBuilder::new().has_headers(true).double_quote(false).escape(Some(b'\\')).from_path(format!("{}/user_ids.csv", root)).unwrap();
            for x in reader.records() {
                let record = x.unwrap();
                let email = String::from(& record[0]);
                let id = record[1].parse::<u64>().unwrap() as UserId;
                assert_eq!(id as usize, result.len());
                result.push(User{
                    id,
                    email,
                    name : String::new()
                });
            }
        }
        // now load the records
        {
            let mut reader = csv::ReaderBuilder::new().has_headers(true).double_quote(false).escape(Some(b'\\')).from_path(format!("{}/user_records.csv", root)).unwrap();
            for x in reader.records() {
                let record = x.unwrap();
                let id = record[1].parse::<u64>().unwrap();
                let name = helpers::decode_quotes(& record[2]);
                result[id as usize].name = name;
            }
        }
        return result;
    }

    fn get_commits(root : & str, num_commits : usize) -> Vec<CommitBase> {
        let mut result = Vec::<CommitBase>::with_capacity(num_commits);
        for _ in 0..num_commits {
            result.push(CommitBase{
                parents : Vec::new(),
                committer_id : 0,
                committer_time : 0,
                author_id : 0,
                author_time : 0,
            })
        }
        {
            let mut reader = csv::ReaderBuilder::new().has_headers(true).double_quote(false).escape(Some(b'\\')).from_path(format!("{}/commit_records.csv", root)).unwrap();
            for x in reader.records() {
                let record = x.unwrap();
                let id = record[1].parse::<usize>().unwrap();
                let ref mut commit = result[id];
                commit.committer_id = record[2].parse::<u64>().unwrap() as UserId;
                commit.committer_time = record[3].parse::<i64>().unwrap();
                commit.author_id = record[4].parse::<u64>().unwrap() as UserId;
                commit.author_time = record[5].parse::<i64>().unwrap();
            }
        }
        // and now load the parents
        let mut parents_update_times = Vec::<u64>::with_capacity(num_commits);
        for _ in 0..num_commits {
            parents_update_times.push(0);
        }
        {
            let mut reader = csv::ReaderBuilder::new().has_headers(true).double_quote(false).escape(Some(b'\\')).from_path(format!("{}/commit_parents.csv", root)).unwrap();
            for x in reader.records() {
                let record = x.unwrap();
                let t = record[0].parse::<u64>().unwrap();
                let id = record[1].parse::<usize>().unwrap();
                // clear the parent records if the update time differs
                if t != parents_update_times[id] {
                    parents_update_times[id] = t;
                    result[id].parents.clear();
                }
                result[id].parents.push(record[2].parse::<u64>().unwrap() as CommitId);
            }
        }
        return result;
    }

    fn get_commit_message_offsets(root : & str) -> HashMap<CommitId, u64> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .double_quote(false)
            .escape(Some(b'\\'))
            .from_path(DatabaseManager::get_commit_messages_index_file(root)).unwrap();
        let mut result = HashMap::<CommitId, u64>::new();
        for x in reader.records() {
            let record = x.unwrap();
            let _t = record[0].parse::<i64>().unwrap();
            let commit_id = record[1].parse::<u64>().unwrap() as CommitId;
            let offset = record[2].parse::<u64>().unwrap();
            result.insert(commit_id, offset);
        }
        return result;
    }

    fn get_commit_change_offsets(root : & str) -> HashMap<CommitId, u64> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .double_quote(false)
            .escape(Some(b'\\'))
            .from_path(DatabaseManager::get_commit_changes_index_file(root)).unwrap();
        let mut result = HashMap::<CommitId, u64>::new();
        for x in reader.records() {
            let record = x.unwrap();
            let _t = record[0].parse::<i64>().unwrap();
            let commit_id = record[1].parse::<u64>().unwrap() as CommitId;
            let offset = record[2].parse::<u64>().unwrap();
            result.insert(commit_id, offset);
        }
        return result;
    }

    fn get_paths(root : & str) -> Vec<String> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .double_quote(false)
            .escape(Some(b'\\'))
            .from_path(DatabaseManager::get_path_ids_file(root)).unwrap();
        let mut result = Vec::<String>::new();
        for x in reader.records() {
            let record = x.unwrap();
            //println!("{:?}", record);
            let id = record[1].parse::<u64>().unwrap();
            assert_eq!(id, result.len() as u64);
            result.push(helpers::decode_quotes(& record[0]));
        }
        return result;
    }



}

impl Database for DCD {

    /** Returns the number of projects the downloader contains.
     */
    fn num_projects(& self) -> u64 {
        return self.num_projects_;
    }

    fn num_commits(& self) -> u64 {
        return self.commit_ids_.len() as u64;
    }

    fn num_users(& self) -> u64 {
        return self.users_.len() as u64;
    }

    fn num_file_paths(& self) -> u64 {
        return self.paths_.len() as u64;
    }
    
    fn get_project(& self, id : ProjectId) -> Option<Project> {
        if let Ok(project) = std::panic::catch_unwind(||{
            return Project::from_log(id, & db_manager::DatabaseManager::get_project_log_file(& self.root_, id), & self);
        }) {
            return project;
        } else {
            return None;
        }
    }

    fn get_commit(& self, id : CommitId) -> Option<Commit> {
        if let Some(base) = self.commits_.get(id as usize) {
            let mut result = Commit::new(id, self.commit_hashes_[& id], base);
            // check lazily for message
            if let Some(offset) = self.commit_message_offsets_.get(& id) {
                let mut messages = self.commit_messages_.lock().unwrap();
                messages.seek(SeekFrom::Start(*offset)).unwrap();
                let commit_id = messages.read_u64::<LittleEndian>().unwrap();
                assert_eq!(id as u64, commit_id);
                let size = messages.read_u32::<LittleEndian>().unwrap();
                let mut buffer = vec![0; size as usize];
                messages.read(&mut buffer).unwrap();
                result.message = Some(buffer);
            }
            // TODO and for changes
            if let Some(offset) = self.commit_change_offsets_.get(&id) {
                let mut commit_changes = self.commit_changes_.lock().unwrap();
                commit_changes.seek(SeekFrom::Start(*offset)).unwrap();
                let commit_id = commit_changes.read_u64::<LittleEndian>().unwrap();
                assert_eq!(id as u64, commit_id);
                let num_changes = commit_changes.read_u32::<LittleEndian>().unwrap() as usize;
                result.additions = Some(commit_changes.read_u64::<LittleEndian>().unwrap());
                result.deletions = Some(commit_changes.read_u64::<LittleEndian>().unwrap());
                let mut changes = HashMap::<PathId, SnapshotId>::new();
                while changes.len() < num_changes  {
                    changes.insert(
                        commit_changes.read_u64::<LittleEndian>().unwrap() as PathId,
                        commit_changes.read_u64::<LittleEndian>().unwrap() as SnapshotId,
                    );
                } 
                result.changes = Some(changes);
            }
            return Some(result);
        } else {
            return None;
        }
    }

    fn get_commit_bare(& self, id : CommitId) -> Option<Commit> {
        if let Some(base) = self.commits_.get(id as usize) {
            let result = Commit::new(id, self.commit_hashes_[& id], base);
            return Some(result);
        } else {
            return None;
        }
    }

    fn get_user(& self, id : UserId) -> Option<&User> {
        return self.users_.get(id as usize);
    }

    fn get_file_path(& self, id : PathId) -> Option<FilePath> {
        match self.paths_.get(id as usize) {
            Some(path) => return Some(FilePath{ id, path : path.to_owned() }),
            None => return None
        }
    }

    // fn commits_from(&self, project: &Project) -> Box<Iterator<Item=Commit>> {
    //     Box::new(ProjectCommitIter::from(self, project))
    // }
    //
    // fn bare_commits_from(&self, project: &Project) -> ProjectBareCommitIter {
    //     ProjectBareCommitIter::from(self, project)
    // }
    //
    // fn user_ids_from(&self, project: &Project) -> ProjectUserIdIter {
    //     ProjectUserIdIter::from(self, project)
    // }
}

impl Source {

    /** Creates source from string.
     */
    pub fn from_str(s : & str) -> Source {
        if *s == *"NA" {
            return Source::NA;
        } else if *s == *"GHT" {
            return Source::GHTorrent;
        } else if *s == *"GH" {
            return Source::GitHub;
        } else if *s == *"M" {
            return Source::Manual;
        } else {
            panic!("Invalid source detected: {}", s);
        }
    }
}



impl std::fmt::Display for Source {
    fn fmt(& self, f : & mut std::fmt::Formatter) -> std::fmt::Result {
        match & self {
            Source::NA => {
                return write!(f, "NA");
            },
            Source::GHTorrent => {
                return write!(f, "GHT");
            },
            Source::GitHub => {
                return write!(f, "GH");
            }
            Source::Manual => {
                return write!(f, "M");
            }
        }
    }
}


impl Project {
    /** Constructs the project information from given log file. 
     */
    fn from_log(id : ProjectId, log_file : & str, dcd : & DCD) -> Option<Project> {
        let mut result = Project{
            id, 
            url : String::new(),
            last_update : 0,
            metadata : HashMap::new(),
            heads : Vec::new(),
        };
        let mut reader = csv::ReaderBuilder::new().has_headers(true).double_quote(false).escape(Some(b'\\')).from_path(log_file).unwrap();
        let mut clear_heads = false;
        for x in reader.records() {
            match record::ProjectLogEntry::from_csv(x.unwrap()) {
                record::ProjectLogEntry::Init{ time : _, source : _, url } => {
                    result.url = url;
                },
                record::ProjectLogEntry::UpdateStart{ time : _, source : _ } => {
                    clear_heads = true;
                },
                record::ProjectLogEntry::Update{ time, source : _ } => {
                    result.last_update = time;
                },
                record::ProjectLogEntry::NoChange{ time, source : _} => {
                    result.last_update = time;
                },
                record::ProjectLogEntry::Metadata{ time : _, source : _, key, value } => {
                    result.metadata.insert(key, value);
                },
                record::ProjectLogEntry::Head{ time : _, source : _, name, hash} => {
                    if clear_heads {
                        result.heads.clear();
                        clear_heads = false;
                    } 
                    result.heads.push((name, dcd.commit_ids_[& hash]));
                }
                record::ProjectLogEntry::Error{ time : _, source : _, message : _ } => {
                    return None;
                }
            }
        }
        return Some(result);
    }
}

impl Commit {
    fn new(id : CommitId, hash: git2::Oid, base : & CommitBase) -> Commit {
        return Commit{
            id : id, 
            hash : hash,
            parents : base.parents.clone(),
            committer_id : base.committer_id,
            committer_time : base.committer_time,
            author_id : base.author_id,
            author_time : base.author_time,
            message : None, 
            changes : None,
            additions : None,
            deletions : None
        };
    } 
}


/** Smaller struct for containing the non-lazy elements of the commit. 
 */
pub struct CommitBase {
    // id of parents
    pub parents : Vec<CommitId>,
    // committer id and time
    pub committer_id : u64,
    pub committer_time : i64,
    // author id and time
    pub author_id : u64,
    pub author_time : i64,
}

// /** Provides methods for iterating over the Database object.
//   */
// trait TraversableDatabase {
//     fn projects(&self) -> ProjectIter;
//     fn commits(&self) -> CommitIter;
//     fn users(&self) -> UserIter;
// }
//
// impl TraversableDatabase for dyn Database {
//
// }

/** Iterates over all projects in the dataset.
 */
pub struct ProjectIter<'a> {
    current:  ProjectId,
    total:    u64,
    database: &'a dyn Database,
}

impl<'a> ProjectIter<'a> {
    pub fn from(database: &impl Database) -> ProjectIter {
        let total = database.num_projects();
        ProjectIter { current: 0, total, database }
    }
}

impl<'a> Iterator for ProjectIter<'a> {
    type Item = Project;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current >= self.total {
                return None;
            }
            if let Some(project) = self.database.get_project(self.current) {
                self.current += 1;
                return Some(project);
            } else {
                self.current += 1;
            }
        }
        // can happen when there are errors in projects
        //panic!("Database returned None for ProjectId={}", self.current); // FIXME maybe better handling
    }
}

/** Iterates over all commits in the dataset.
 */
pub struct CommitIter<'a> {
    current:  CommitId,
    total:    u64,
    database: &'a dyn Database,
}

impl<'a> CommitIter<'a> {
    pub fn from(database: &impl Database) -> CommitIter {
        let total = database.num_commits();
        CommitIter { current: 0, total, database }
    }
}

impl<'a> Iterator for CommitIter<'a> {
    type Item = Commit;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current >= self.total {
                return None;
            }
            if let Some(commit) = self.database.get_commit(self.current) {
                self.current += 1;
                return Some(commit);
            } else {
                self.current += 1;
            }
        }
        // database actually can do that if projects have errors
        //panic!("Database returned None for CommitId={}", self.current); // FIXME maybe better handling
    }
}

/** Iterates over all commits in the dataset, the commits do not have messages or changes.
 */
pub struct BareCommitIter<'a> {
    current:  CommitId,
    total:    u64,
    database: &'a dyn Database,
}

impl<'a> BareCommitIter<'a> {
    pub fn from(database: &impl Database) -> BareCommitIter {
        let total = database.num_commits();
        BareCommitIter { current: 0, total, database }
    }
}

impl<'a> Iterator for BareCommitIter<'a> {
    type Item = Commit;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current >= self.total {
                return None;
            }
            if let Some(commit) = self.database.get_commit_bare(self.current) {
                self.current += 1;
                return Some(commit);
            } else {
                self.current += 1;
            }
        }
        // database actually can do that if projects have errors
        //panic!("Database returned None for CommitId={}", self.current); // FIXME maybe better handling
    }
}

/** Iterates over all users in the dataset.
 */
pub struct UserIter<'a> {
    current:  UserId,
    total:    u64,
    database: &'a dyn Database,
}

impl<'a> UserIter<'a> {
    pub fn from(database: &impl Database) -> UserIter {
        UserIter {
            current: 0,
            total: database.num_users(),
            database,
        }
    }
}

impl<'a> Iterator for UserIter<'a> {
    type Item = &'a User;

    fn next(&mut self) -> Option<Self::Item> {
        if !(self.current < self.total) {
            return None;
        }

        if let Some(user) = self.database.get_user(self.current) {
            self.current += 1;
            return Some(user);
        }

        panic!("Database returned None for UserId={}", self.current); // FIXME maybe better handling
    }
}

// rolled into CommitIter/BareCommitIter
// pub struct FastProjectCommitIter<'a> {
//     visited : HashSet<CommitId>,
//     q : VecDeque<CommitId>,
//     db : &'a dyn Database,
// }
//
// impl<'a> FastProjectCommitIter<'a> {
//     pub fn from(database: &'a impl Database, project: &Project) -> FastProjectCommitIter<'a> {
//         return FastProjectCommitIter {
//             visited : HashSet::new(),
//             q : project.heads.iter().map(|(_, id)| *id).collect(),
//             db : database,
//         };
//     }
// }
//
// impl<'a> Iterator for FastProjectCommitIter<'a> {
//     type Item = Commit;
//
//     fn next(& mut self) -> Option<Self::Item> {
//         // nothing in the queue, we are done
//         loop {
//             if self.q.is_empty() {
//                 return None;
//             }
//             let commit_id = self.q.pop_back().unwrap();
//             if ! self.visited.insert(commit_id) {
//                 continue;
//             }
//             let commit = self.db.get_commit_bare(commit_id).unwrap();
//             self.q.extend(commit.parents.iter());
//             return Some(commit);
//         }
//     }
// }

/** Iterates over all commits within a specific project.
 */
pub struct ProjectCommitIter<'a> {
    visited:  HashSet<CommitId>,
    to_visit: VecDeque<CommitId>,
    database: &'a dyn Database,
}

impl<'a> ProjectCommitIter<'a> {
    pub fn from(database: &'a impl Database, project: &Project) -> ProjectCommitIter<'a> {
        let visited: HashSet<CommitId> = HashSet::new();
        let to_visit: VecDeque<CommitId> = project.heads.iter().map(|(_, id)| *id).collect();
        ProjectCommitIter { visited, to_visit, database }
    }
}

impl<'a> Iterator for ProjectCommitIter<'a> {
    type Item = Commit;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.to_visit.is_empty() {
                return None;
            }
            let commit_id = self.to_visit.pop_back().unwrap();
            if ! self.visited.insert(commit_id) {
                continue;
            }
            let commit = self.database.get_commit(commit_id).unwrap();
            self.to_visit.extend(commit.parents.iter());
            return Some(commit);
        }
    }
}

/** Iterates over all commits within a specific project.
 
    This time returns bare commit objects, i.e. the precached commits with no message or changes information. 
 */
pub struct ProjectBareCommitIter<'a> {
    visited:  HashSet<CommitId>,
    to_visit: VecDeque<CommitId>,
    database: &'a dyn Database,
}

impl<'a> ProjectBareCommitIter<'a> {
    pub fn from(database: &'a impl Database, project: &Project) -> ProjectBareCommitIter<'a> {
        let visited: HashSet<CommitId> = HashSet::new();
        let to_visit: VecDeque<CommitId> = project.heads.iter().map(|(_, id)| *id).collect();
        ProjectBareCommitIter { visited, to_visit, database }
    }
}

impl<'a> Iterator for ProjectBareCommitIter<'a> {
    type Item = Commit;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.to_visit.is_empty() {
                return None;
            }
            let commit_id = self.to_visit.pop_back().unwrap();
            if ! self.visited.insert(commit_id) {
                continue;
            }
            let commit = self.database.get_commit_bare(commit_id).unwrap();
            self.to_visit.extend(commit.parents.iter());
            return Some(commit);
        }
    }
}

pub struct ProjectUserIdIter<'a> {
    commit_iter: ProjectCommitIter<'a>,
    seen_users: HashSet<UserId>,
    user_cache: HashSet<UserId>,
    desired_cache_size: usize,
}

impl<'a> ProjectUserIdIter<'a> {
    pub fn from(database: &'a impl Database, project: &Project) -> ProjectUserIdIter<'a> {
        let desired_cache_size = 100usize;
        let seen_users: HashSet<CommitId> = HashSet::new();
        let user_cache: HashSet<CommitId> = HashSet::with_capacity(desired_cache_size);
        let commit_iter = ProjectCommitIter::from(database, project);
        ProjectUserIdIter { commit_iter, seen_users, user_cache, desired_cache_size }
    }

    fn next_from_cache(&mut self) -> Option<UserId> {
        let user_id = self.user_cache.iter().next().map(|u| *u); // Blergh...

        if let Some(user_id) = user_id {
            self.user_cache.remove(&user_id); // There are only unseen user_ids in cache.
            return Some(user_id)
        }

        return None
    }

    fn populate_cache(&mut self) -> bool {
        loop {
            return match self.commit_iter.next() {
                Some(commit) => {
                    if self.seen_users.insert(commit.author_id) {
                        self.user_cache.insert(commit.author_id); // User not yet seen.
                    }

                    if self.seen_users.insert(commit.committer_id) {
                        self.user_cache.insert(commit.committer_id); // User not yet seen.
                    }

                    if self.user_cache.len() < self.desired_cache_size {
                        continue;
                    }

                    true
                },
                None => self.user_cache.len() != 0
            }
        }
    }
}

impl<'a> Iterator for ProjectUserIdIter<'a> {
    type Item = UserId;

     fn next(&mut self) -> Option<Self::Item> {
        loop {
            let user_opt = self.next_from_cache();

            if user_opt.is_some() {
                return user_opt
            }

            if !self.populate_cache() {
                return None
            }
        }
    }
}

/**
 * Guaranteed to provide Projects from lowest to highiest ID.
 */
pub struct ProjectAllCommitIdsIter<'a, D> where D: Database + Sized {
    current:  ProjectId,
    total:    u64,
    database: &'a D,
}

impl<'a, D> ProjectAllCommitIdsIter<'a, D> where D: Database + Sized {
    pub fn from(database: &'a D) -> Self {
        let total = database.num_projects();
        ProjectAllCommitIdsIter{ current: 0, total, database }
    }
}

impl<'a, D> Iterator for ProjectAllCommitIdsIter<'a, D> where D: Database + Sized {
    type Item = (ProjectId, Vec<CommitId>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current >= self.total {
                return None;
            }
            if let Some(project) = self.database.get_project(self.current) {
                self.current += 1;
                let commits = self.database.bare_commits_from(&project);
                return Some((project.id, commits.map(|c| c.id).collect()));
            } else {
                self.current += 1;
            }
        }
        // can happen when there are errors in projects
        //panic!("Database returned None for ProjectId={}", self.current); // FIXME maybe better handling
    }
}
