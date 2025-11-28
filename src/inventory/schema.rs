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

diesel::table! {
    task_queue (id) {
        id -> Text,
        drive_id -> Text,
        task_type -> Text,
        local_path -> Text,
        remote_uri -> Nullable<Text>,
        status -> Text,
        progress -> Double,
        total_bytes -> BigInt,
        processed_bytes -> BigInt,
        priority -> Integer,
        custom_state -> Nullable<Text>,
        error -> Nullable<Text>,
        created_at -> BigInt,
        updated_at -> BigInt,
    }
}
