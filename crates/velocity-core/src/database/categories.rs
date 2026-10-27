//! Categories in the database (#31): the built-in ones from schema 6 and the user's. Names are
//! unique whatever their case. Deleting a category leaves its downloads without one and touches
//! no file.

use rusqlite::{params, Connection, OptionalExtension, Row};

use super::{Database, DatabaseError};
use crate::category::{Category, CategoryInput};
use crate::domain::ErrorCode;

const COLUMNS: &str = "id, name, color, icon, extensions_json, directory, builtin";

impl Database {
    /// Every category, in the order the automatic category is chosen in.
    pub fn categories(&self) -> Result<Vec<Category>, DatabaseError> {
        all(&self.conn())
    }

    /// The category `id`, or `None`.
    pub fn category(&self, id: &str) -> Result<Option<Category>, DatabaseError> {
        one(&self.conn(), id)
    }

    /// Adds a user category after the others, with the next free `c-<n>` id. `NameTaken` when
    /// another category has the name, whatever its case.
    pub fn create_category(&self, input: &CategoryInput) -> Result<Category, DatabaseError> {
        let input = checked(input)?;
        let conn = self.conn();
        refuse_taken(&conn, &input.name, None)?;
        let (last, order): (i64, i64) = conn.query_row(
            "SELECT COALESCE(MAX(CASE WHEN id GLOB 'c-[0-9]*' THEN CAST(substr(id, 3) AS INTEGER) END), 0),
                    COALESCE(MAX(sort_order), 0)
             FROM categories",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let id = format!("c-{}", last + 1);
        conn.execute(
            "INSERT INTO categories (id, name, color, icon, extensions_json, directory, builtin, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
            params![
                id,
                input.name,
                input.color,
                input.icon,
                extensions_json(&input)?,
                input.directory,
                order + 1
            ],
        )
        .map_err(taken)?;
        one(&conn, &id)?.ok_or_else(|| DatabaseError::new(ErrorCode::Internal, "created category"))
    }

    /// Changes a category, built-in or not. `NotFound` or `NameTaken`.
    pub fn update_category(
        &self,
        id: &str,
        input: &CategoryInput,
    ) -> Result<Category, DatabaseError> {
        let input = checked(input)?;
        let conn = self.conn();
        if one(&conn, id)?.is_none() {
            return Err(DatabaseError::new(ErrorCode::NotFound, "the category"));
        }
        refuse_taken(&conn, &input.name, Some(id))?;
        conn.execute(
            "UPDATE categories SET name = ?2, color = ?3, icon = ?4, extensions_json = ?5,
                                   directory = ?6
             WHERE id = ?1",
            params![
                id,
                input.name,
                input.color,
                input.icon,
                extensions_json(&input)?,
                input.directory
            ],
        )
        .map_err(taken)?;
        one(&conn, id)?.ok_or_else(|| DatabaseError::new(ErrorCode::NotFound, "the category"))
    }

    /// Deletes a user category; its downloads are left without one. `NotFound`, or
    /// `InvalidState` for a built-in category.
    pub fn delete_category(&self, id: &str) -> Result<(), DatabaseError> {
        let conn = self.conn();
        match one(&conn, id)? {
            None => Err(DatabaseError::new(ErrorCode::NotFound, "the category")),
            Some(c) if c.builtin => Err(DatabaseError::new(
                ErrorCode::InvalidState,
                "a built-in category can't be deleted",
            )),
            Some(_) => {
                // `downloads.category_id` is `ON DELETE SET NULL`.
                conn.execute("DELETE FROM categories WHERE id = ?1", [id])?;
                Ok(())
            }
        }
    }
}

fn checked(input: &CategoryInput) -> Result<CategoryInput, DatabaseError> {
    input
        .checked()
        .map_err(|code| DatabaseError::new(code, "the category"))
}

fn extensions_json(input: &CategoryInput) -> Result<String, DatabaseError> {
    serde_json::to_string(&input.extensions)
        .map_err(|e| DatabaseError::new(ErrorCode::Internal, format!("the extensions: {e}")))
}

fn all(conn: &Connection) -> Result<Vec<Category>, DatabaseError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM categories ORDER BY sort_order, name"
    ))?;
    let rows = statement.query_map([], from_row)?;
    rows.map(|row| row.map_err(DatabaseError::from).and_then(|r| r))
        .collect()
}

fn one(conn: &Connection, id: &str) -> Result<Option<Category>, DatabaseError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM categories WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .transpose()
}

/// `NameTaken` when a category other than `except` has `name`, comparing them lowercased
/// (SQLite's `NOCASE` folds only ASCII letters).
fn refuse_taken(conn: &Connection, name: &str, except: Option<&str>) -> Result<(), DatabaseError> {
    let lower = name.to_lowercase();
    let clash = all(conn)?
        .iter()
        .any(|c| Some(c.id.as_str()) != except && c.name.to_lowercase() == lower);
    if clash {
        return Err(DatabaseError::new(
            ErrorCode::NameTaken,
            "the category name",
        ));
    }
    Ok(())
}

/// The table's `UNIQUE` name, should it catch what [`refuse_taken`] didn't.
fn taken(e: rusqlite::Error) -> DatabaseError {
    match e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            DatabaseError::new(ErrorCode::NameTaken, "the category name")
        }
        other => other.into(),
    }
}

/// A row, or a `Database` error when its extensions aren't a list of strings.
fn from_row(r: &Row<'_>) -> rusqlite::Result<Result<Category, DatabaseError>> {
    let extensions: String = r.get(4)?;
    let category = Category {
        id: r.get(0)?,
        name: r.get(1)?,
        color: r.get(2)?,
        icon: r.get(3)?,
        extensions: Vec::new(),
        directory: r.get(5)?,
        builtin: r.get::<_, i64>(6)? != 0,
    };
    Ok(serde_json::from_str(&extensions)
        .map(|extensions| Category {
            extensions,
            ..category
        })
        .map_err(|e| {
            DatabaseError::new(ErrorCode::Database, format!("a category's extensions: {e}"))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str) -> CategoryInput {
        CategoryInput {
            name: name.to_owned(),
            color: "#112233".to_owned(),
            icon: "folder".to_owned(),
            extensions: vec![".ABC".to_owned(), "abc".to_owned()],
            directory: None,
        }
    }

    fn absolute(rest: &str) -> String {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        format!("{root}{rest}")
    }

    fn code<T: std::fmt::Debug>(r: Result<T, DatabaseError>) -> ErrorCode {
        r.expect_err("refused").code
    }

    #[test]
    fn the_built_in_categories_come_first() {
        let db = Database::open_in_memory().unwrap();
        let ids: Vec<String> = db.categories().unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "compressed",
                "documents",
                "music",
                "video",
                "programs",
                "images"
            ]
        );
        let video = db.category("video").unwrap().expect("built in");
        assert!(video.builtin);
        assert!(video.extensions.contains(&"mkv".to_owned()));
        assert_eq!(db.category("c-404").unwrap(), None);
    }

    #[test]
    fn a_user_category_is_created_edited_and_deleted() {
        let db = Database::open_in_memory().unwrap();
        let mine = db.create_category(&input("Mine")).unwrap();
        assert_eq!(mine.id, "c-1");
        assert_eq!(mine.extensions, vec!["abc"], "stored as checked");
        assert!(!mine.builtin);
        assert_eq!(
            db.categories().unwrap().last(),
            Some(&mine),
            "after the others"
        );
        assert_eq!(db.create_category(&input("Two")).unwrap().id, "c-2");

        let renamed = db
            .update_category(
                "c-1",
                &CategoryInput {
                    directory: Some(absolute("Mine")),
                    ..input("Renamed")
                },
            )
            .unwrap();
        assert_eq!(renamed.name, "Renamed");
        assert_eq!(db.category("c-1").unwrap(), Some(renamed));

        db.delete_category("c-1").unwrap();
        assert_eq!(db.category("c-1").unwrap(), None);
        assert_eq!(db.create_category(&input("Three")).unwrap().id, "c-3");
    }

    #[test]
    fn names_are_unique_whatever_their_case() {
        let db = Database::open_in_memory().unwrap();
        db.create_category(&input("Mine")).unwrap();
        assert_eq!(
            code(db.create_category(&input("mine"))),
            ErrorCode::NameTaken
        );
        assert_eq!(
            code(db.create_category(&input("VIDEO"))),
            ErrorCode::NameTaken
        );
        assert_eq!(
            code(db.update_category("c-1", &input("Documents"))),
            ErrorCode::NameTaken
        );
        // Its own name, in another case, is not taken.
        db.update_category("c-1", &input("MINE")).unwrap();
        // Not only ASCII letters: SQLite's NOCASE alone would let this through.
        db.create_category(&input("Курсы")).unwrap();
        assert_eq!(
            code(db.create_category(&input("курсы"))),
            ErrorCode::NameTaken
        );
    }

    #[test]
    fn what_cant_be_changed_is_refused() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(
            code(db.create_category(&CategoryInput {
                color: "red".to_owned(),
                ..input("A")
            })),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            code(db.update_category("c-9", &input("A"))),
            ErrorCode::NotFound
        );
        assert_eq!(code(db.delete_category("c-9")), ErrorCode::NotFound);
        assert_eq!(code(db.delete_category("video")), ErrorCode::InvalidState);
        // A built-in category can be edited.
        let edited = db.update_category("video", &input("Films")).unwrap();
        assert!(edited.builtin);
        assert_eq!(db.categories().unwrap().len(), 6);
    }

    #[test]
    fn a_deleted_categorys_downloads_are_left_without_one() {
        let db = Database::open_in_memory().unwrap();
        let mine = db.create_category(&input("Mine")).unwrap();
        let mut record = super::super::tests::sample("d-0001");
        record.category_id = Some(mine.id.clone());
        db.insert_download(&record).unwrap();
        db.delete_category(&mine.id).unwrap();
        assert_eq!(db.downloads().unwrap()[0].category_id, None);
    }
}
