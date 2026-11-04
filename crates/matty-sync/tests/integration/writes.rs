//! Commands that change the server: posts, reactions, read state, drafts,
//! typing and status.

use std::time::Duration;

use matty_api::ws::{WsConnection, WsMessage};
use matty_sync::{ChangeSet, Command};

use crate::world::{Engine, World, eventually};

#[tokio::test(flavor = "multi_thread")]
async fn a_post_is_sent_edited_and_deleted() {
    let Some(world) = World::new().await else {
        return;
    };
    let engine = Engine::start(&world.me).await;
    let channel_id = world.channel.id.clone();
    engine.open(&channel_id, |_| true).await;

    engine.send(Command::SendPost {
        pending_post_id: String::new(),
        channel_id: channel_id.clone(),
        root_id: String::new(),
        message: "writes: hello".into(),
        file_ids: Vec::new(),
        props: Default::default(),
    });
    // The optimistic row, then the server's.
    let window = engine
        .open(&channel_id, |posts| {
            posts
                .iter()
                .any(|p| p.message == "writes: hello" && p.id != p.pending_post_id)
        })
        .await;
    let sent = window
        .iter()
        .find(|p| p.message == "writes: hello")
        .unwrap()
        .clone();
    let on_server = world.other.client.get_post(&sent.id).await.unwrap();
    assert_eq!(on_server.message, "writes: hello");
    assert_eq!(on_server.user_id, world.me.id);

    engine.send(Command::EditPost {
        post_id: sent.id.clone(),
        message: "writes: edited".into(),
    });
    engine
        .open(&channel_id, |posts| {
            posts
                .iter()
                .any(|p| p.id == sent.id && p.message == "writes: edited")
        })
        .await;
    let client = world.other.client.clone();
    eventually("the edit on the server", || {
        let client = client.clone();
        let id = sent.id.clone();
        async move { client.get_post(&id).await.unwrap().message == "writes: edited" }
    })
    .await;

    engine.send(Command::DeletePost {
        post_id: sent.id.clone(),
    });
    engine
        .open(&channel_id, |posts| {
            posts.iter().all(|p| p.id != sent.id || p.delete_at > 0)
        })
        .await;
    eventually("the delete on the server", || {
        let client = client.clone();
        let id = sent.id.clone();
        async move { client.get_post(&id).await.is_err() }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reaction_toggles_on_and_off() {
    let Some(world) = World::new().await else {
        return;
    };
    let post = world.post(&world.other, "writes: react to me", "").await;
    let engine = Engine::start(&world.me).await;
    engine
        .open(&world.channel.id, |posts| {
            posts.iter().any(|p| p.id == post.id)
        })
        .await;
    for on in [true, false] {
        engine.send(Command::ToggleReaction {
            post_id: post.id.clone(),
            emoji_name: "+1".into(),
        });
        let mine = |reactions: &[matty_api::models::Reaction]| {
            reactions
                .iter()
                .any(|r| r.user_id == world.me.id && r.emoji_name == "+1")
        };
        // At once in the window, then on the server.
        engine
            .open(&world.channel.id, |posts| {
                posts
                    .iter()
                    .any(|p| p.id == post.id && mine(&p.metadata.reactions) == on)
            })
            .await;
        eventually("the reaction on the server", || {
            let client = world.other.client.clone();
            let id = post.id.clone();
            async move { mine(&client.get_post(&id).await.unwrap().metadata.reactions) == on }
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn viewing_reads_and_marking_unread_unreads() {
    let Some(world) = World::new().await else {
        return;
    };
    let first = world.post(&world.other, "writes: one", "").await;
    world.post(&world.other, "writes: two", "").await;
    let engine = Engine::start(&world.me).await;
    let channel_id = world.channel.id.clone();
    engine
        .sidebar(&channel_id, |c| c.unreads.messages == 2)
        .await;

    let unread_on_server = || {
        let client = world.me.client.clone();
        let channel_id = channel_id.clone();
        async move {
            let member = client.my_channel_member(&channel_id).await.unwrap();
            let channel = client.channel(&channel_id).await.unwrap();
            channel.total_msg_count - member.msg_count
        }
    };

    engine.send(Command::ViewChannel {
        channel_id: channel_id.clone(),
        prev_channel_id: String::new(),
    });
    engine
        .sidebar(&channel_id, |c| c.unreads.messages == 0)
        .await;
    eventually("the channel read on the server", || {
        let unread = unread_on_server();
        async move { unread.await == 0 }
    })
    .await;

    engine.send(Command::MarkUnread {
        post_id: first.id.clone(),
    });
    engine
        .sidebar(&channel_id, |c| c.unreads.messages == 2)
        .await;
    eventually("the channel unread on the server", || {
        let unread = unread_on_server();
        async move { unread.await == 2 }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn drafts_sync_when_the_server_allows_them() {
    let Some(world) = World::new().await else {
        return;
    };
    let engine = Engine::start(&world.me).await;
    let synced = world.me.client.capabilities().await.unwrap().synced_drafts;
    let has_draft = |message: &'static str| {
        let client = world.me.client.clone();
        let (me, team_id) = (world.me.id.clone(), world.team_id.clone());
        let channel_id = world.channel.id.clone();
        async move {
            client
                .drafts(&me, &team_id)
                .await
                .unwrap()
                .iter()
                .any(|d| d.channel_id == channel_id && d.message == message)
        }
    };
    for message in ["writes: a draft", ""] {
        engine.send(Command::SaveDraft {
            channel_id: world.channel.id.clone(),
            root_id: String::new(),
            message: message.into(),
            file_ids: Vec::new(),
        });
        // Stored here whatever the server allows.
        engine
            .wait("the draft", |change| {
                matches!(change, ChangeSet::Draft { draft, .. }
                    if draft.as_ref().map(|d| d.message.as_str()).unwrap_or("") == message)
            })
            .await;
        if synced {
            eventually("the draft on the server", || {
                let saved = has_draft("writes: a draft");
                async move { saved.await == !message.is_empty() }
            })
            .await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn status_and_typing_reach_other_users() {
    let Some(world) = World::new().await else {
        return;
    };
    let engine = Engine::start(&world.me).await;
    for status in ["dnd", "online"] {
        engine.send(Command::SetStatus {
            status: status.into(),
            dnd_end_time: None,
        });
        eventually("the status on the server", || {
            let client = world.other.client.clone();
            let me = world.me.id.clone();
            async move { client.statuses(&[me]).await.unwrap()[0].status == status }
        })
        .await;
    }

    // Typing twice in a row reaches the other user once.
    let (socket, mut events) = WsConnection::connect(&world.other.client).await.unwrap();
    for _ in 0..2 {
        engine.send(Command::Typing {
            channel_id: world.channel.id.clone(),
            parent_id: String::new(),
        });
    }
    let mut notices = 0;
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(message) = events.recv().await {
            if let WsMessage::Event(raw) = message
                && raw.event == "typing"
                && raw.data["user_id"] == world.me.id.as_str()
            {
                notices += 1;
            }
        }
    })
    .await;
    assert_eq!(notices, 1);
    drop(socket);
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_messages_open_and_channels_are_left_and_joined() {
    let Some(world) = World::new().await else {
        return;
    };
    let engine = Engine::start(&world.me).await;
    let channel_id = world.channel.id.clone();
    engine.sidebar(&channel_id, |_| true).await;

    // A direct message is created on the server and lands in the sidebar.
    engine.send(Command::OpenDirect {
        user_ids: vec![world.other.id.clone()],
    });
    let ready = engine
        .wait("the direct message", |c| {
            matches!(c, ChangeSet::ChannelReady { .. })
        })
        .await;
    let ChangeSet::ChannelReady { channel_id: direct } = ready else {
        unreachable!()
    };
    let on_server = world.me.client.channel(&direct).await.unwrap();
    assert_eq!(on_server.kind, "D");
    assert!(on_server.name.contains(&world.other.id));
    engine.sidebar(&direct, |_| true).await;

    // Leaving takes the channel out of the sidebar; joining brings it back.
    engine.send(Command::LeaveChannel {
        channel_id: channel_id.clone(),
    });
    engine
        .wait("the channel gone from the sidebar", |change| {
            matches!(change, ChangeSet::Sidebar { team_id, sections }
                if *team_id == world.team_id
                    && sections.iter().all(|s| s.channels.iter().all(|c| c.id != channel_id)))
        })
        .await;
    assert!(
        world
            .me
            .client
            .my_channel_member(&channel_id)
            .await
            .is_err()
    );

    engine.send(Command::JoinChannel {
        channel_id: channel_id.clone(),
    });
    engine
        .wait(
            "the joined channel",
            |c| matches!(c, ChangeSet::ChannelReady { channel_id: id } if *id == channel_id),
        )
        .await;
    engine
        .wait("the channel back in the sidebar", |change| {
            matches!(change, ChangeSet::Sidebar { sections, .. }
                if sections.iter().any(|s| s.channels.iter().any(|c| c.id == channel_id)))
        })
        .await;
    world
        .me
        .client
        .my_channel_member(&channel_id)
        .await
        .unwrap();
}
