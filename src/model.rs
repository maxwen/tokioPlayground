use diesel::{Insertable, Queryable, Selectable};

#[derive(Queryable, Selectable, Debug)]
#[diesel(table_name = crate::schema::history)]
pub struct History {
    pub id: i32,
    pub title: String,
    pub station: String,
    pub timestamp: String
}

#[derive(Insertable, Debug)]
#[diesel(table_name = crate::schema::history)]
pub struct NewHistory<'a> {
    pub title: &'a str,
    pub station: &'a str,
    pub timestamp: &'a str,
}