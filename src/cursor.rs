/*
** Copyright (c) 2011, Brian Smith <brian@linuxfood.net>
** All rights reserved.
**
** Redistribution and use in source and binary forms, with or without
** modification, are permitted provided that the following conditions are met:
**
**   * Redistributions of source code must retain the above copyright notice,
**     this list of conditions and the following disclaimer.
**
**   * Redistributions in binary form must reproduce the above copyright notice,
**     this list of conditions and the following disclaimer in the documentation
**     and/or other materials provided with the distribution.
**
**   * Neither the name of Brian Smith nor the names of its contributors
**     may be used to endorse or promote products derived from this software
**     without specific prior written permission.
**
** THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
** AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
** IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
** ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
** LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
** CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
** SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
** INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
** CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
** ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
** POSSIBILITY OF SUCH DAMAGE.
*/

use ffi::*;
use libc::{c_int, c_void, c_char};
use std::collections::HashMap;
use std::mem::transmute;
use std::str;
use std::slice;
use std::c_str::CString;
use types::*;

/// The database cursor.
///
/// Unlike `Database`, `Cursor` is dependent of the active database connection
/// and cannot be shared across tasks (non-`Send`able). It is a compile time error
/// that `Cursor` outlives `Database` either by `Database` escaping the block scope
/// or by the task-shared reference to `Database` being expired.
pub struct Cursor<'db> {
    stmt: *mut stmt,
    _dbh: &'db *mut dbh // make this non-`Send`able
}

pub fn cursor_with_statement<'db>(stmt: *mut stmt, dbh: &'db *mut dbh) -> Cursor<'db> {
    debug!("`Cursor.cursor_with_statement()`: stmt={}", stmt);
    Cursor { stmt: stmt, _dbh: dbh }
}

#[unsafe_destructor]
impl<'db> Drop for Cursor<'db> {
    /// Deletes a prepared SQL statement.
    /// See http://www.sqlite.org/c3ref/finalize.html
    fn drop(&mut self) {
        debug!("`Cursor.drop()`: stmt={}", self.stmt);
        unsafe {
            sqlite3_finalize(self.stmt);
        }
    }
}

impl<'db> Cursor<'db> {

    /// Resets a prepared SQL statement, but does not reset its bindings.
    /// See http://www.sqlite.org/c3ref/reset.html
    pub fn reset(&mut self) -> ResultCode {
        unsafe {
            sqlite3_reset(self.stmt)
        }
    }

    /// Resets all bindings on a prepared SQL statement.
    /// See http://www.sqlite.org/c3ref/clear_bindings.html
    pub fn clear_bindings(&mut self) -> ResultCode {
        unsafe {
            sqlite3_clear_bindings(self.stmt)
        }
    }

    /// Evaluates a prepared SQL statement one ore more times.
    /// See http://www.sqlite.org/c3ref/step.html
    pub fn step(&mut self) -> ResultCode {
        unsafe {
            sqlite3_step(self.stmt)
        }
    }

    ///
    pub fn step_row(&mut self) -> SqliteResult<Option<RowMap>> {
        let is_row: ResultCode = self.step();
        if is_row == SQLITE_ROW {
            let column_cnt = self.get_column_count();
            let mut i = 0;
            let mut sqlrow = HashMap::new();
            while i < column_cnt {
                let name = self.get_column_name(i).to_string();
                let coltype = self.get_column_type(i);
                let res = match coltype {
                    SQLITE_INTEGER => sqlrow.insert(name, Integer(self.get_int(i))),
                    SQLITE_FLOAT   => sqlrow.insert(name, Float64(self.get_f64(i))),
                    SQLITE_TEXT    => sqlrow.insert(name, Text(self.get_text(i).unwrap().to_string())),
                    SQLITE_BLOB    => sqlrow.insert(name, Blob(self.get_blob(i).unwrap().to_vec())),
                    SQLITE_NULL    => sqlrow.insert(name, Null),
                };
                assert!(res.is_none(), "Duplicate column name for sqlrow!");
                i += 1;
            }

            Ok(Some(sqlrow))
        }
        else if is_row == SQLITE_DONE {
            Ok(None)
        } else {
            Err(is_row)
        }
    }

    ///
    /// See http://www.sqlite.org/c3ref/column_blob.html
    pub fn get_blob<'a>(&'a mut self, i: int) -> Option<&'a [u8]> {
        let ptr = unsafe {sqlite3_column_blob(self.stmt, i as c_int)};
        let len = unsafe {sqlite3_column_bytes(self.stmt, i as c_int)} as uint;
        if ptr.is_null() {
            None
        } else {
            // make `bytes` outlive the `buf_as_slice` call
            unsafe {slice::raw::buf_as_slice(ptr, len, |bytes: &[u8]| Some(transmute(bytes)))}
        }
    }

    ///
    /// See http://www.sqlite.org/c3ref/column_blob.html
    pub fn get_int(&mut self, i: int) -> int {
        unsafe {
            return sqlite3_column_int(self.stmt, i as c_int) as int;
        }
    }

    ///
    /// See http://www.sqlite.org/c3ref/column_blob.html
    pub fn get_i64(&mut self, i: int) -> i64 {
        unsafe {
            return sqlite3_column_int64(self.stmt, i as c_int) as i64;
        }
    }

    ///
    /// See http://www.sqlite.org/c3ref/column_blob.html
    pub fn get_f64(&mut self, i: int) -> f64 {
        unsafe {
            return sqlite3_column_double(self.stmt, i as c_int);
        }
    }

    ///
    /// See http://www.sqlite.org/c3ref/column_blob.html
    pub fn get_text<'a>(&'a mut self, i: int) -> Option<&'a str> {
        let ptr = unsafe {sqlite3_column_text(self.stmt, i as c_int)};
        let len = unsafe {sqlite3_column_bytes(self.stmt, i as c_int)} as uint;
        if ptr.is_null() {
            None
        } else {
            unsafe {
                slice::raw::buf_as_slice(ptr as *const u8, len, |bytes| {
                    let text: &str = str::raw::from_utf8(bytes);
                    Some(transmute(text)) // make `text` outlive the `buf_as_slice` call
                })
            }
        }
    }

    ///
    /// See http://www.sqlite.org/c3ref/bind_parameter_index.html
    pub fn get_bind_index(&self, name: &str) -> int {
        let stmt = self.stmt;
        unsafe {
            name.with_c_str( |_name| {
              return sqlite3_bind_parameter_index(stmt, _name) as int
            })
        }
    }

    /// Returns the number of columns in a result set.
    /// See http://www.sqlite.org/c3ref/data_count.html
    pub fn get_column_count(&self) -> int {
        unsafe {
            return sqlite3_data_count(self.stmt) as int;
        }
    }

    /// Returns the name of the column with index `i` in the result set.
    /// See http://www.sqlite.org/c3ref/column_name.html
    pub fn get_column_name<'a>(&'a self, i: int) -> &'a str {
        unsafe {
            let name = CString::new(sqlite3_column_name(self.stmt, i as c_int), false);
            let namestr: &str = str::raw::from_utf8(name.as_bytes_no_nul());
            transmute(namestr) // make it outlive the original `CString`
        }
    }

    /// Returns the type of the column with index `i` in the result set.
    /// See http://www.sqlite.org/c3ref/column_blob.html
    pub fn get_column_type(&self, i: int) -> ColumnType {
        let ct;
        unsafe {
            ct = sqlite3_column_type(self.stmt, i as c_int) as int;
        }
        let res = match ct {
            1 /* SQLITE_INTEGER */ => SQLITE_INTEGER,
            2 /* SQLITE_FLOAT   */ => SQLITE_FLOAT,
            3 /* SQLITE_TEXT    */ => SQLITE_TEXT,
            4 /* SQLITE_BLOB    */ => SQLITE_BLOB,
            5 /* SQLITE_NULL    */ => SQLITE_NULL,
            _ => panic!(format!("sqlite internal error: Got an unknown column type ({:d}) back from the library.", ct)),
        };
        return res;
    }

    /// Returns the names of all columns in the result set.
    pub fn get_column_names(&self) -> Vec<String> {
        let cnt = self.get_column_count();
        let mut i = 0;
        let mut r = Vec::new();
        while i < cnt {
            r.push(self.get_column_name(i).to_string());
            i += 1;
        }
        return r;
    }

    ///
    pub fn bind_params(&mut self, values: &[BindArg]) -> ResultCode {
        // SQL parameter index (starting from 1).
        let mut i = 1i;
        for v in values.iter() {
            let r = self.bind_param(i, v);
            if r != SQLITE_OK {
                return r;
            }
            i += 1;
        }
        return SQLITE_OK;
    }

    ///
    /// See http://www.sqlite.org/c3ref/bind_blob.html
    pub fn bind_param(&mut self, i: int, value: &BindArg) -> ResultCode {

        debug!("`Cursor.bind_param()`: stmt={}", self.stmt);

        let r = match *value {
            Text(ref v) => {
                let l = v.len();
                debug!("  `Text`: v={}, l={}", v, l);

                (*v).with_c_str( |_v| {
                    debug!("  _v={}", _v);
                    unsafe {
                        // FIXME: do not copy the data
                        sqlite3_bind_text(
                              self.stmt   // the SQL statement
                            , i as c_int  // the SQL parameter index (starting from 1)
                            , _v          // the value to bind
                            , l as c_int  // the number of bytes
                            , -1 as *mut c_void// SQLITE_TRANSIENT => SQLite makes a copy
                            )
                    }
                })
            }

            StaticText(ref v) => {
                let l = v.len();
                debug!("  `StaticText`: v={}, l={}", v, l);

                {
                    let _v = v.as_bytes();
                    debug!("  _v={}", _v);
                    unsafe {
                        sqlite3_bind_text(
                              self.stmt   // the SQL statement
                            , i as c_int  // the SQL parameter index (starting from 1)
                            , _v.as_ptr() as *const c_char // the value to bind
                            , l as c_int  // the number of bytes
                            , 0 as *mut c_void// SQLITE_STATIC
                            )
                    }
                }
            }

            Blob(ref v) => {
                let l = v.len();
                debug!("`Blob`: v={}, l={}", v, l);

                unsafe {
                    // FIXME: do not copy the data
                    sqlite3_bind_blob(
                          self.stmt   // the SQL statement
                        , i as c_int  // the SQL parameter index (starting from 1)
                        , v.as_ptr()  // the value to bind
                        , l as c_int  // the number of bytes
                        , -1 as *mut c_void // SQLITE_TRANSIENT => SQLite makes a copy
                        )
                }
            }

            Integer(ref v) => { unsafe { sqlite3_bind_int(self.stmt, i as c_int, *v as c_int) } }

            Integer64(ref v) => { unsafe { sqlite3_bind_int64(self.stmt, i as c_int, *v) } }

            Float64(ref v) => { unsafe { sqlite3_bind_double(self.stmt, i as c_int, *v) } }

            Null => { unsafe { sqlite3_bind_null(self.stmt, i as c_int) } }

        };

        return r;
    }
}
