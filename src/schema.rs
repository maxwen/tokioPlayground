// @generated automatically by Diesel CLI.

diesel::table! {
    history (id) {
        id -> Integer,
        title -> Text,
        station -> Text,
        timestamp -> Text,
    }
}
