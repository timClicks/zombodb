use lazy_static::*;
use memoffset::*;
use pgx::*;
use std::ffi::{CStr, CString};

const DEFAULT_BATCH_SIZE: i32 = 8 * 1024 * 1024;
const DEFAULT_COMPRESSION_LEVEL: i32 = 1;
const DEFAULT_SHARDS: i32 = 5;
const DEFAULT_OPTIMIZE_AFTER: i32 = 0;
const DEFAULT_URL: &str = "default";
const DEFAULT_TYPE_NAME: &str = "doc";
const DEFAULT_REFRESH_INTERVAL: &str = "-1";

lazy_static! {
    static ref DEFAULT_BULK_CONCURRENCY: i32 = num_cpus::get() as i32;
}

#[repr(C)]
pub struct ZDBIndexOptions {
    /* varlena header (do not touch directly!) */
    #[allow(dead_code)]
    vl_len_: i32,

    url_offset: i32,
    type_name_offset: i32,
    refresh_interval_offset: i32,
    alias_offset: i32,
    uuid_offset: i32,

    optimize_after: i32,
    compression_level: i32,
    shards: i32,
    replicas: i32,
    bulk_concurrency: i32,
    batch_size: i32,
    llapi: bool,
}

#[allow(dead_code)]
impl ZDBIndexOptions {
    pub unsafe fn from(relation: &PgBox<pg_sys::RelationData>) -> PgBox<ZDBIndexOptions> {
        if relation.rd_index.is_null() {
            panic!("relation doesn't represent an index")
        } else if relation.rd_options.is_null() {
            // use defaults
            let mut ops = PgBox::<ZDBIndexOptions>::alloc0();
            ops.compression_level = DEFAULT_COMPRESSION_LEVEL;
            ops.shards = DEFAULT_SHARDS;
            ops.replicas = ZDB_DEFAULT_REPLICAS_GUC;
            ops.bulk_concurrency = *DEFAULT_BULK_CONCURRENCY;
            ops.batch_size = DEFAULT_BATCH_SIZE;
            ops.optimize_after = DEFAULT_OPTIMIZE_AFTER;
            ops
        } else {
            PgBox::from_pg(relation.rd_options as *mut ZDBIndexOptions)
        }
    }

    pub fn optimize_after(&self) -> i32 {
        self.optimize_after
    }

    pub fn compression_level(&self) -> i32 {
        self.compression_level
    }

    pub fn shards(&self) -> i32 {
        self.shards
    }

    pub fn replicas(&self) -> i32 {
        self.replicas
    }

    pub fn bulk_concurrency(&self) -> i32 {
        self.bulk_concurrency
    }

    pub fn batch_size(&self) -> i32 {
        self.batch_size
    }

    pub fn llapi(&self) -> bool {
        self.llapi
    }

    pub fn url(&self) -> String {
        if self.url_offset == 0 {
            DEFAULT_URL.to_owned()
        } else {
            self.get_str(self.url_offset).unwrap()
        }
    }

    pub fn type_name(&self) -> String {
        if self.type_name_offset == 0 {
            DEFAULT_TYPE_NAME.to_owned()
        } else {
            self.get_str(self.type_name_offset).unwrap()
        }
    }

    pub fn refresh_interval(&self) -> String {
        if self.refresh_interval_offset == 0 {
            DEFAULT_REFRESH_INTERVAL.to_owned()
        } else {
            self.get_str(self.refresh_interval_offset).unwrap()
        }
    }

    pub fn alias(
        &self,
        heaprel: &PgBox<pg_sys::RelationData>,
        indexrel: &PgBox<pg_sys::RelationData>,
    ) -> String {
        match self.get_str(self.alias_offset) {
            Some(alias) => alias.to_owned(),
            None => format!(
                "{}.{}.{}.{}-{}",
                unsafe {
                    std::ffi::CStr::from_ptr(pg_sys::get_database_name(pg_sys::MyDatabaseId))
                }
                .to_str()
                .unwrap(),
                unsafe {
                    std::ffi::CStr::from_ptr(pg_sys::get_namespace_name(
                        relation_get_namespace_oid(indexrel),
                    ))
                }
                .to_str()
                .unwrap(),
                relation_get_relation_name(heaprel),
                relation_get_relation_name(indexrel),
                relation_get_id(indexrel)
            ),
        }
    }

    pub fn uuid(
        &self,
        heaprel: &PgBox<pg_sys::RelationData>,
        indexrel: &PgBox<pg_sys::RelationData>,
    ) -> String {
        match self.get_str(self.uuid_offset) {
            Some(uuid) => uuid,
            None => format!(
                "{}.{}.{}.{}",
                unsafe { pg_sys::MyDatabaseId },
                relation_get_namespace_oid(indexrel),
                relation_get_id(heaprel),
                relation_get_id(indexrel),
            ),
        }
    }

    pub fn index_name(
        &self,
        heaprel: &PgBox<pg_sys::RelationData>,
        indexrel: &PgBox<pg_sys::RelationData>,
    ) -> String {
        self.uuid(heaprel, indexrel)
    }

    fn get_str(&self, offset: i32) -> Option<String> {
        if offset == 0 {
            None
        } else {
            let opts = self as *const _ as void_ptr as usize;
            let value =
                unsafe { CStr::from_ptr((opts + offset as usize) as *const std::os::raw::c_char) };

            Some(value.to_str().unwrap().to_owned())
        }
    }
}

static ZDB_DEFAULT_REPLICAS_GUC: i32 = 0;
static mut RELOPT_KIND_ZDB: pg_sys::relopt_kind = 0;

extern "C" fn validate_url(url: *const std::os::raw::c_char) {
    let url = unsafe { CStr::from_ptr(url) }
        .to_str()
        .expect("failed to convert url to utf8");

    if url == "default" {
        // "default" is a fine value
        return;
    }

    if !url.ends_with('/') {
        panic!("url must end with a forward slash");
    }

    if let Err(e) = url::Url::parse(url) {
        panic!(e.to_string())
    }
}

#[pg_guard]
pub unsafe extern "C" fn amoptions(
    reloptions: pg_sys::Datum,
    validate: bool,
) -> *mut pg_sys::bytea {
    // TODO:  how to make this const?  we can't use offset_of!() macro in const definitions, apparently
    let tab: [pg_sys::relopt_parse_elt; 12] = [
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"url\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_STRING,
            offset: offset_of!(ZDBIndexOptions, url_offset) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"type_name\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_STRING,
            offset: offset_of!(ZDBIndexOptions, type_name_offset) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"refresh_interval\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_STRING,
            offset: offset_of!(ZDBIndexOptions, refresh_interval_offset) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"shards\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_INT,
            offset: offset_of!(ZDBIndexOptions, shards) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"replicas\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_INT,
            offset: offset_of!(ZDBIndexOptions, replicas) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"bulk_concurrency\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_INT,
            offset: offset_of!(ZDBIndexOptions, bulk_concurrency) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"batch_size\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_INT,
            offset: offset_of!(ZDBIndexOptions, batch_size) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"compression_level\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_INT,
            offset: offset_of!(ZDBIndexOptions, compression_level) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"alias\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_STRING,
            offset: offset_of!(ZDBIndexOptions, alias_offset) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"optimize_after\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_INT,
            offset: offset_of!(ZDBIndexOptions, optimize_after) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"llapi\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_BOOL,
            offset: offset_of!(ZDBIndexOptions, llapi) as i32,
        },
        pg_sys::relopt_parse_elt {
            optname: CStr::from_bytes_with_nul_unchecked(b"uuid\0").as_ptr(),
            opttype: pg_sys::relopt_type_RELOPT_TYPE_STRING,
            offset: offset_of!(ZDBIndexOptions, uuid_offset) as i32,
        },
    ];

    let mut noptions = 0;
    let options = pg_sys::parseRelOptions(reloptions, validate, RELOPT_KIND_ZDB, &mut noptions);
    if noptions == 0 {
        return std::ptr::null_mut();
    }

    for relopt in std::slice::from_raw_parts_mut(options, noptions as usize) {
        relopt.gen.as_mut().unwrap().lockmode = pg_sys::AccessShareLock as pg_sys::LOCKMODE;
    }

    let rdopts =
        pg_sys::allocateReloptStruct(std::mem::size_of::<ZDBIndexOptions>(), options, noptions);
    pg_sys::fillRelOptions(
        rdopts,
        std::mem::size_of::<ZDBIndexOptions>(),
        options,
        noptions,
        validate,
        tab.as_ptr(),
        tab.len() as i32,
    );
    pg_sys::pfree(options as void_mut_ptr);

    rdopts as *mut pg_sys::bytea
}

pub unsafe fn init() {
    RELOPT_KIND_ZDB = pg_sys::add_reloption_kind();
    pg_sys::add_string_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"url\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"Server URL and port\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"default\0").as_ptr(),
        Some(validate_url),
    );
    pg_sys::add_string_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"type_name\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(
            b"What Elasticsearch index type name should ZDB use?  Default is 'doc'\0",
        )
        .as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"doc\0").as_ptr(),
        None,
    );
    pg_sys::add_string_reloption(RELOPT_KIND_ZDB, CStr::from_bytes_with_nul_unchecked(b"refresh_interval\0").as_ptr(),
                                 CStr::from_bytes_with_nul_unchecked(b"Frequency in which Elasticsearch indexes are refreshed.  Related to ES' index.refresh_interval setting\0").as_ptr(),
                                 CString::new(DEFAULT_REFRESH_INTERVAL).unwrap().as_ptr(), None);
    pg_sys::add_int_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"shards\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"The number of shards for the index\0").as_ptr(),
        DEFAULT_SHARDS,
        1,
        32768,
    );
    pg_sys::add_int_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"replicas\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"The number of replicas for the index\0").as_ptr(),
        ZDB_DEFAULT_REPLICAS_GUC,
        0,
        32768,
    );
    pg_sys::add_int_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"bulk_concurrency\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(
            b"The maximum number of concurrent _bulk API requests\0",
        )
        .as_ptr(),
        *DEFAULT_BULK_CONCURRENCY,
        1,
        num_cpus::get() as i32,
    );
    pg_sys::add_int_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"batch_size\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"The size in bytes of batch calls to the _bulk API\0")
            .as_ptr(),
        DEFAULT_BATCH_SIZE,
        1,
        (std::i32::MAX / 2) - 1,
    );
    pg_sys::add_int_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"compression_level\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(
            b"0-9 value to indicate the level of HTTP compression\0",
        )
        .as_ptr(),
        DEFAULT_COMPRESSION_LEVEL,
        0,
        9,
    );
    pg_sys::add_string_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"alias\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(
            b"The Elasticsearch Alias to which this index should belong\0",
        )
        .as_ptr(),
        std::ptr::null(),
        None,
    );
    pg_sys::add_string_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"uuid\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(b"The Elasticsearch index name, as a UUID\0").as_ptr(),
        std::ptr::null(),
        None,
    );
    pg_sys::add_int_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"optimize_after\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(
            b"After how many deleted docs should ZDB _optimize the ES index during VACUUM?\0",
        )
        .as_ptr(),
        DEFAULT_OPTIMIZE_AFTER,
        0,
        std::i32::MAX,
    );
    pg_sys::add_bool_reloption(
        RELOPT_KIND_ZDB,
        CStr::from_bytes_with_nul_unchecked(b"llapi\0").as_ptr(),
        CStr::from_bytes_with_nul_unchecked(
            b"Will this index be used by ZomboDB's low-level API?\0",
        )
        .as_ptr(),
        false,
    );
}

#[cfg(any(test, feature = "pg_test"))]
mod tests {
    use crate::access_method::options::{
        validate_url, ZDBIndexOptions, DEFAULT_BATCH_SIZE, DEFAULT_BULK_CONCURRENCY,
        DEFAULT_COMPRESSION_LEVEL, DEFAULT_OPTIMIZE_AFTER, DEFAULT_REFRESH_INTERVAL,
        DEFAULT_SHARDS, DEFAULT_TYPE_NAME, DEFAULT_URL, ZDB_DEFAULT_REPLICAS_GUC,
    };
    use pgx::*;
    use std::ffi::CString;

    #[test]
    fn make_idea_happy() {}

    #[pg_test]
    fn test_validate_url() {
        validate_url(CString::new("http://localhost:9200/").unwrap().as_ptr());
    }

    #[pg_test]
    fn test_validate_default_url() {
        validate_url(CString::new("default").unwrap().as_ptr());
    }

    #[pg_test(error = "url must end with a forward slash")]
    fn test_validate_invalid_url() {
        validate_url(CString::new("http://localhost:9200").unwrap().as_ptr());
    }

    #[pg_test]
    unsafe fn test_index_options() {
        Spi::run(
            "CREATE TABLE test();  
        CREATE INDEX idxtest 
                  ON test 
               USING zombodb ((test.*)) 
                WITH (url='http://localhost:9200/', 
                      type_name='test_type_name', 
                      alias='test_alias', 
                      uuid='test_uuid', 
                      refresh_interval='5s'); ",
        );

        let heap_oid = Spi::get_one::<pg_sys::Oid>("SELECT 'test'::regclass::oid")
            .expect("failed to get SPI result");
        let index_oid = Spi::get_one::<pg_sys::Oid>("SELECT 'idxtest'::regclass::oid")
            .expect("failed to get SPI result");
        let heaprel = PgBox::from_pg(pg_sys::RelationIdGetRelation(heap_oid));
        let indexrel = PgBox::from_pg(pg_sys::RelationIdGetRelation(index_oid));
        let options = ZDBIndexOptions::from(&indexrel);
        assert_eq!(&options.url(), "http://localhost:9200/");
        assert_eq!(&options.type_name(), "test_type_name");
        assert_eq!(&options.alias(&heaprel, &indexrel), "test_alias");
        assert_eq!(&options.uuid(&heaprel, &indexrel), "test_uuid");
        assert_eq!(&options.refresh_interval(), "5s");
        assert_eq!(options.compression_level(), 1);
        assert_eq!(options.shards(), 5);
        assert_eq!(options.replicas(), 0);
        assert_eq!(options.bulk_concurrency(), num_cpus::get() as i32);
        assert_eq!(options.batch_size(), 8 * 1024 * 1024);
        assert_eq!(options.optimize_after(), DEFAULT_OPTIMIZE_AFTER);
        assert_eq!(options.llapi(), false);
        pg_sys::RelationClose(indexrel.into_pg());
    }

    #[pg_test]
    unsafe fn test_index_options_defaults() {
        Spi::run(
            "CREATE TABLE test();  
        CREATE INDEX idxtest 
                  ON test 
               USING zombodb ((test.*));",
        );

        let heap_oid = Spi::get_one::<pg_sys::Oid>("SELECT 'test'::regclass::oid")
            .expect("failed to get SPI result");
        let index_oid = Spi::get_one::<pg_sys::Oid>("SELECT 'idxtest'::regclass::oid")
            .expect("failed to get SPI result");
        let heaprel = PgBox::from_pg(pg_sys::RelationIdGetRelation(heap_oid));
        let indexrel = PgBox::from_pg(pg_sys::RelationIdGetRelation(index_oid));
        let options = ZDBIndexOptions::from(&indexrel);
        assert_eq!(&options.url(), DEFAULT_URL);
        assert_eq!(&options.type_name(), DEFAULT_TYPE_NAME);
        assert_eq!(
            &options.alias(&heaprel, &indexrel),
            &format!(
                "pgx_tests.public.test.idxtest-{}",
                relation_get_id(&indexrel)
            )
        );
        assert_eq!(
            &options.uuid(&heaprel, &indexrel),
            &format!(
                "{}.{}.{}.{}",
                pg_sys::MyDatabaseId,
                relation_get_namespace_oid(&indexrel),
                relation_get_id(&heaprel),
                relation_get_id(&indexrel)
            )
        );
        assert_eq!(&options.refresh_interval(), DEFAULT_REFRESH_INTERVAL);
        assert_eq!(options.compression_level(), DEFAULT_COMPRESSION_LEVEL);
        assert_eq!(options.shards(), DEFAULT_SHARDS);
        assert_eq!(options.replicas(), ZDB_DEFAULT_REPLICAS_GUC);
        assert_eq!(options.bulk_concurrency(), *DEFAULT_BULK_CONCURRENCY);
        assert_eq!(options.batch_size(), DEFAULT_BATCH_SIZE);
        assert_eq!(options.optimize_after(), DEFAULT_OPTIMIZE_AFTER);
        assert_eq!(options.llapi(), false);
        pg_sys::RelationClose(indexrel.into_pg());
    }
}