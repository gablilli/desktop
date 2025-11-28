// @generated automatically by Diesel CLI.
diesel::table! {
    file_metadata (id) {
        id -> BigInt,
        drive_id -> Text,
        is_folder -> Bool,
        local_path -> Text,
        remote_uri -> Text,
        created_at -> BigInt,
        updated_at -> BigInt,
        etag -> Text,
        metadata -> Text,
        props -> Nullable<Text>,
        permissions -> Text,
        shared -> Bool,
        size -> BigInt,
    }
}
