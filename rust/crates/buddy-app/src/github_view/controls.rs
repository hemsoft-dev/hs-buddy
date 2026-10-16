use super::*;
use buddy_core::github::{Reaction, RepositoryRef};

impl GitHubView {
    pub(super) fn route_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let routes: Vec<(Route, &'static str)> = match &self.route {
            Route::Pull(pull, _) => [
                (PullTab::Conversation, "Conversation"),
                (PullTab::Reviews, "Reviews"),
                (PullTab::Commits, "Commits"),
                (PullTab::Checks, "Checks"),
                (PullTab::Files, "Files"),
                (PullTab::Threads, "Review threads"),
                (PullTab::History, "History"),
            ]
            .into_iter()
            .map(|(tab, label)| (Route::Pull(pull.clone(), tab), label))
            .collect(),
            Route::Repository(repo, _) => [
                (RepoTab::Overview, "Overview"),
                (RepoTab::Pulls, "Open PRs"),
                (RepoTab::ClosedPulls, "Closed PRs"),
                (RepoTab::Issues, "Open issues"),
                (RepoTab::ClosedIssues, "Closed issues"),
                (RepoTab::Commits, "Commits"),
            ]
            .into_iter()
            .map(|(tab, label)| (Route::Repository(repo.clone(), tab), label))
            .collect(),
            Route::User(login, _) => [
                (UserTab::Overview, "Profile"),
                (UserTab::Authored, "Authored PRs"),
                (UserTab::Reviewed, "Reviewed PRs"),
                (UserTab::Activity, "Activity"),
                (UserTab::Teams, "Team memberships"),
            ]
            .into_iter()
            .map(|(tab, label)| (Route::User(login.clone(), tab), label))
            .collect(),
            _ => Vec::new(),
        };
        h_flex()
            .flex_wrap()
            .gap_2()
            .children(
                routes
                    .into_iter()
                    .enumerate()
                    .map(|(index, (route, label))| {
                        Button::new(format!("github-tab-{index}"))
                            .label(label)
                            .selected(self.route == route)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.navigate(route.clone(), cx)),
                            )
                    }),
            )
    }

    pub(super) fn actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut actions = v_flex().gap_2();
        if matches!(self.route, Route::Pulls(_)) {
            actions = actions.child(
                h_flex().gap_2().children(
                    [(ViewMode::Card, "Cards"), (ViewMode::List, "List")]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (mode, label))| {
                            Button::new(format!("github-view-{index}"))
                                .label(label)
                                .selected(self.view_mode == mode)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.choose_view(mode, cx)),
                                )
                        }),
                ),
            );
        }
        if let Some(data) = &self.data
            && let Some(pull) = &data.pull
        {
            if let Some(resource) = self.current_write_key() {
                for ((key, _), status) in &self.review_status {
                    if key == &resource {
                        actions = actions.child(div().whitespace_normal().child(status.clone()));
                    }
                }
            }
            actions = actions.child(div().child(format!(
                "{} · {} · {} → {} · {} files · {} threads · head {}",
                pull.state,
                pull.author,
                pull.head_branch,
                pull.base_branch,
                pull.changed_files,
                pull.thread_count,
                pull.head_sha
            )));
            let target = pull.target.clone();
            let url = target.web_url();
            let repo = target.repository.clone();
            let author = pull.author.clone();
            actions = actions.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("github-open-pull")
                            .label("Open on GitHub")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(
                        Button::new("github-pull-repo")
                            .label("Repository")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.navigate(
                                    Route::Repository(repo.clone(), RepoTab::Overview),
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new("github-pull-author")
                            .label(format!("@{author}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.navigate(Route::User(author.clone(), UserTab::Overview), cx)
                            })),
                    ),
            );
            let disabled =
                self.write_busy || self.must_reload || self.loading || self.pending.is_some();
            if let Some(account) = self.account.clone() {
                let client = self.clients.for_account(account);
                let target = target.clone();
                actions = actions.child(
                    Button::new("github-native-assistant-review")
                        .label("Review with Buddy assistant…")
                        .disabled(disabled)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(super::GitHubEvent::PrepareAssistantReview(
                                client.clone(),
                                target.clone(),
                            ))
                        })),
                );
            }
            let approve = WriteAction::Approve {
                pull: target.clone(),
                head_sha: pull.head_sha.clone(),
            };
            let copilot = WriteAction::RequestCopilot {
                pull: target.clone(),
            };
            let rabbit = WriteAction::RequestCodeRabbit {
                pull: target.clone(),
            };
            actions =
                actions.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("github-approve")
                                .label("Approve…")
                                .disabled(disabled || pull.state != "OPEN")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.prepare(approve.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new("github-request-copilot")
                                .label("Request Copilot review…")
                                .disabled(disabled || pull.state != "OPEN")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.prepare(copilot.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new("github-request-coderabbit")
                                .label("Request CodeRabbit…")
                                .disabled(
                                    disabled
                                        || data.coderabbit != Some(true)
                                        || pull.state != "OPEN",
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.prepare(rabbit.clone(), cx)
                                })),
                        ),
                );
            actions=actions.child(div().text_color(cx.theme().muted_foreground).child(match data.coderabbit {Some(true)=>"Review requests are submitted to GitHub. Check Reviews and Conversation for the resulting review.",Some(false)=>"No .coderabbit.yaml configuration was found in this repository.",None=>"CodeRabbit configuration could not be checked. Refresh to retry."}));
            actions = actions.child(
                Button::new("github-nudge-start")
                    .label("Nudge author…")
                    .disabled(disabled || pull.state != "OPEN")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nudge_editing = true;
                        cx.notify();
                    })),
            );
            if self.nudge_editing {
                actions = actions
                    .child("Author's Slack email")
                    .child(Input::new(&self.nudge_email))
                    .child(
                        Button::new("github-nudge-lookup")
                            .label("Look up Slack recipient…")
                            .disabled(
                                disabled
                                    || self
                                        .current_write_key()
                                        .is_some_and(|key| self.unknown_nudges.contains(&key)),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.lookup_nudge(cx))),
                    );
            }
            if matches!(self.route, Route::Pull(_, PullTab::Conversation)) {
                actions = actions
                    .child("PR comment")
                    .child(
                        Textarea::new(&self.draft)
                            .h(zpx(100.0))
                            .disabled(self.write_busy),
                    )
                    .child(
                        Button::new("github-comment")
                            .label("Review comment…")
                            .disabled(disabled || self.draft.read(cx).value().trim().is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prepare(
                                    WriteAction::Comment {
                                        pull: target.clone(),
                                        body: this.draft.read(cx).value().to_string(),
                                    },
                                    cx,
                                )
                            })),
                    );
            }
        }
        match &self.route {
            Route::Repository(repo, _) => {
                let target = repo.clone();
                let bookmarked = self.bookmarks.as_ref().is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item.target().ok().as_ref() == Some(repo))
                });
                let url = repo.web_url();
                actions = actions.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("github-open-repository")
                                .label("Open on GitHub")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                        .child(
                            Button::new("github-bookmark")
                                .label(if bookmarked {
                                    "Remove bookmark"
                                } else {
                                    "Bookmark repository"
                                })
                                .disabled(self.write_busy || self.bookmarks.is_none())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.bookmark(target.clone(), cx)
                                })),
                        ),
                );
            }
            Route::User(login, _) => {
                let login = login.clone();
                let url = format!("https://github.com/{login}");
                actions = actions.child(
                    Button::new("github-open-user")
                        .label("Open profile on GitHub")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
                let favorite = Settings::global(cx)
                    .config
                    .ui
                    .favorite_users
                    .iter()
                    .any(|user| user.eq_ignore_ascii_case(&login));
                actions = actions.child(
                    Button::new("github-favorite-user")
                        .label(if favorite {
                            "Remove favorite"
                        } else {
                            "Favorite user"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if Settings::update(cx, |config| {
                                if favorite {
                                    config
                                        .ui
                                        .favorite_users
                                        .retain(|user| !user.eq_ignore_ascii_case(&login));
                                } else if !config
                                    .ui
                                    .favorite_users
                                    .iter()
                                    .any(|user| user.eq_ignore_ascii_case(&login))
                                {
                                    config.ui.favorite_users.push(login.clone());
                                }
                            }) {
                                this.status = Some("Favorite saved".into());
                            }
                            cx.notify();
                        })),
                );
            }
            Route::Thread(thread) => {
                let thread = thread.0.clone();
                actions = actions
                    .child(
                        Textarea::new(&self.draft)
                            .h(zpx(100.0))
                            .disabled(self.write_busy),
                    )
                    .child(
                        Button::new("github-reply")
                            .label("Review reply…")
                            .disabled(
                                self.write_busy
                                    || self.must_reload
                                    || self.loading
                                    || self.draft.read(cx).value().trim().is_empty(),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.prepare(
                                    WriteAction::Reply {
                                        thread: thread.clone(),
                                        body: this.draft.read(cx).value().to_string(),
                                    },
                                    cx,
                                )
                            })),
                    );
            }
            Route::Issue(repo, number) => {
                let url = format!("{}/issues/{number}", repo.web_url());
                actions = actions.child(
                    Button::new("github-open-issue")
                        .label("Open issue on GitHub")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            Route::Commit(repo, sha) => {
                let url = format!("{}/commit/{sha}", repo.web_url());
                actions = actions.child(
                    Button::new("github-open-commit")
                        .label("Open commit on GitHub")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            Route::Members => {
                actions = actions.child(
                    h_flex().flex_wrap().gap_2().children(
                        Settings::global(cx)
                            .config
                            .ui
                            .favorite_users
                            .iter()
                            .enumerate()
                            .map(|(index, login)| {
                                let login = login.clone();
                                Button::new(format!("github-favorite-{index}"))
                                    .label(format!("Favorite: @{login}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.navigate(
                                            Route::User(login.clone(), UserTab::Overview),
                                            cx,
                                        )
                                    }))
                            }),
                    ),
                );
            }
            _ => {}
        }
        actions
    }

    pub(super) fn row(
        &self,
        index: usize,
        row: &data::Row,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut item = v_flex()
            .gap_2()
            .min_w_0()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border);
        let card = matches!(self.route, Route::Pulls(_)) && self.view_mode == ViewMode::Card;
        if card {
            item = item
                .p_3()
                .border_1()
                .rounded(zpx(8.0))
                .bg(cx.theme().secondary);
        }
        if let Some(route) = &row.route {
            let route = route.clone();
            item = item.child(
                Button::new(format!("github-item-{index}"))
                    .label(row.title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(route.clone(), cx))),
            );
        } else {
            item = item.child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(row.title.clone()),
            );
        }
        item = item.child(
            div()
                .text_color(if card {
                    cx.theme().secondary_foreground.opacity(0.8)
                } else {
                    cx.theme().muted_foreground
                })
                .whitespace_normal()
                .child(row.detail.clone()),
        );
        if let Some(body) = &row.body {
            let mut text = div().whitespace_normal().child(body.clone());
            if matches!(
                self.route,
                Route::Pull(_, PullTab::Files) | Route::Commit(_, _)
            ) {
                text = text.font_family(cx.theme().mono_font_family.clone());
            }
            item = item.child(text);
        }
        let mut links = h_flex().flex_wrap().gap_2();
        if let Some(author) = &row.author {
            let login = author.clone();
            links = links.child(
                Button::new(format!("github-item-author-{index}"))
                    .label(format!("@{login}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate(Route::User(login.clone(), UserTab::Overview), cx)
                    })),
            );
        }
        if let Some(url) = &row.url {
            let url = url.clone();
            links = links.child(
                Button::new(format!("github-item-url-{index}"))
                    .label("Open link")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        if let Some(thread) = &row.thread {
            let thread = thread.clone();
            let resolved = !row.resolved;
            links = links.child(
                Button::new(format!("github-thread-resolve-{index}"))
                    .label(if resolved { "Resolve…" } else { "Reopen…" })
                    .disabled(self.write_busy || self.must_reload || self.loading)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.prepare(
                            WriteAction::Resolve {
                                thread: thread.clone(),
                                resolved,
                            },
                            cx,
                        )
                    })),
            );
        }
        if let Some(comment) = &row.comment {
            for (reaction_index, reaction) in Reaction::ALL.into_iter().enumerate() {
                let comment = comment.clone();
                links = links.child(
                    Button::new(format!("github-reaction-{index}-{reaction_index}"))
                        .label(reaction.label())
                        .disabled(self.write_busy || self.must_reload || self.loading)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.prepare(
                                WriteAction::React {
                                    comment: comment.clone(),
                                    reaction,
                                },
                                cx,
                            )
                        })),
                );
            }
        }
        item.child(links)
    }

    fn bookmark(&mut self, target: RepositoryRef, cx: &mut Context<Self>) {
        if self.write_busy {
            return;
        }
        self.write_busy = true;
        let url = buddy_core::convex_data::convex_url();
        let folder = target.owner().to_string();
        let existing = self
            .bookmarks
            .as_ref()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.target().ok().as_ref() == Some(&target))
            })
            .map(|item| item.id.clone());
        let removed = existing.is_some();
        let rx = Runtime::global(cx).spawn(async move {
            let result = if let Some(id) = existing {
                github::backend::remove_bookmark(&url, &id)
                    .await
                    .map(|()| true)
            } else {
                github::backend::add_bookmark(&url, &target, &folder)
                    .await
                    .map(|receipt| receipt.stats_updated)
            };
            let refreshed = github::backend::bookmarks(&url).await;
            (result, refreshed)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.write_busy = false;
                this.status = Some(match result {
                    Ok((Ok(stats), refreshed)) => {
                        this.bookmarks = refreshed.ok();
                        if removed {
                            "Repository bookmark removed".into()
                        } else if stats {
                            "Repository bookmarked".into()
                        } else {
                            "Repository bookmarked; statistics could not be updated.".into()
                        }
                    }
                    Ok((Err(error), refreshed)) => {
                        this.bookmarks = refreshed.ok();
                        error.to_string()
                    }
                    Err(_) => {
                        "The bookmark result is unknown. Reload bookmarks before retrying.".into()
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn contributions(
        &self,
        days: &[github::ContributionDay],
        cx: &App,
    ) -> impl IntoElement + use<> {
        use gpui_kit::StatefulInteractiveElement as _;
        use gpui_kit::component::tooltip::Tooltip;
        let accent = cx.theme().primary;
        let empty = cx.theme().secondary;
        let maximum = days.iter().map(|day| day.count).max().unwrap_or(1).max(1) as f32;
        v_flex()
            .gap_2()
            .child("Contribution calendar · hover a day for its date and count")
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(days.chunks(7).enumerate().map(|(week, days)| {
                        v_flex().gap_1().children(days.iter().enumerate().map(
                            move |(day_index, day)| {
                                let label = format!("{}: {} contributions", day.date, day.count);
                                div()
                                    .id(format!("github-contribution-{week}-{day_index}"))
                                    .w(zpx(10.0))
                                    .h(zpx(10.0))
                                    .bg(if day.count == 0 {
                                        empty
                                    } else {
                                        accent.opacity(
                                            0.35 + 0.65 * (day.count as f32 / maximum).sqrt(),
                                        )
                                    })
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(label.clone()).build(window, cx)
                                    })
                            },
                        ))
                    })),
            )
    }
}
