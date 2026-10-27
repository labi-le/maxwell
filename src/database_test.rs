use crate::prelude::*;
use crate::*;


    #[tokio::test]
    async fn group_toggle_survives_reopening_database() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("state.db").display());
        let pool = open_database(&url).await.unwrap();
        let chat = ChatId(-100123);
        assert!(!group_enabled(&pool, chat).await.unwrap());
        set_group_enabled(&pool, chat, true).await.unwrap();
        pool.close().await;
        let pool = open_database(&url).await.unwrap();
        assert!(group_enabled(&pool, chat).await.unwrap());
        set_group_enabled(&pool, chat, false).await.unwrap();
        assert!(!group_enabled(&pool, chat).await.unwrap());
    }
