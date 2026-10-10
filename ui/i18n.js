"use strict";
// Languages. The interface is written in American English; that text is also the key each
// translation is found by, so adding a string means writing it once in the code and once per
// language here. `tx("Archive {n} conversations", { n })` gives the text in the language in
// use, with the braces filled in.
//
// The server speaks English too (what it says about a sender, card labels, errors). Those
// sentences are translated here as well: fixed ones by the same lookup, ones with a name or
// number in them by the patterns at the end.

const LOCALES = { "en-US": "English (US)", "en-GB": "English (UK)", "ja-JP": "日本語" };

/** The language to use: the one chosen in the app, else the closest to the system's. */
const LOCALE = (() => {
  let saved = null;
  try { saved = localStorage.getItem("locale"); } catch {}
  if (saved in LOCALES) return saved;
  const sys = (navigator.language || "en-US").toLowerCase();
  if (sys.startsWith("ja")) return "ja-JP";
  // American spelling for the US (and the Philippines, which follows it); British for the rest
  if (sys.startsWith("en")) return sys === "en-us" || sys === "en-ph" ? "en-US" : "en-GB";
  return "en-US";
})();
document.documentElement.lang = LOCALE.slice(0, 2);

const MESSAGES = {
  // The server writes a few words the British way.
  "en-US": {
    "Cancelled": "Canceled",
    "Car hire": "Car rental",
  },

  "en-GB": {
    "Color": "Colour",
    "Appearance": "Appearance",
    "Choose the language and the app's color": "Choose the language and the app's colour",
    "The color used across Unibox. Its brightness is adjusted where needed so it stays readable in light and dark.":
      "The colour used across Unibox. Its brightness is adjusted where needed so it stays readable in light and dark.",
    "Color as #rrggbb": "Colour as #rrggbb",
    "Organizer": "Organiser",
    "Canceled": "Cancelled",
  },

  "ja-JP": {
    // ---- sidebar
    "All inboxes": "すべての受信トレイ",
    "{email}: sync problem: {error}": "{email}: 同期の問題: {error}",
    "Remove this account from Unibox": "このアカウントを Unibox から削除",
    "Change the app password": "アプリ パスワードを変更",
    "Can't sync {accounts}": "{accounts} を同期できません",
    "Synced {when}": "同期: {when}",
    "Syncing…": "同期中…",
    "Remove {email} from Unibox?\n\nThis deletes the copy of its mail on this PC and the saved app password. Nothing changes in Gmail.":
      "{email} を Unibox から削除しますか?\n\nこの PC 上のメールのコピーと保存済みのアプリ パスワードが削除されます。Gmail 側は何も変わりません。",
    "Add a Gmail account": "Gmail アカウントを追加",
    "Add account": "アカウントを追加",
    "Subscriptions": "配信登録",
    "Doctor": "診断",
    "What Unibox is connected to, what it is doing, and what it keeps on this PC.": "Unibox の接続先、現在の動作、この PC に保存しているもの。",
    "Listing older mail…": "古いメールを一覧に追加中…",
    "Doctor: connections, activity and disk space (g then d)": "診断: 接続状況、動作の記録、ディスク使用量 (g の後に d)",
    "Doctor: connections, activity and disk space": "診断: 接続状況、動作の記録、ディスク使用量",
    "Activity": "動作の記録",
    "Which activity to show": "表示する記録",
    "Everything": "すべて",
    "Gmail": "Gmail",
    "Other assistants": "他のアシスタント",
    "App": "アプリ",
    "Only problems": "問題のみ",
    "Commands, sizes and timings since Unibox started. The mail itself is never recorded here, and nothing is kept after you close the app.": "Unibox の起動後に送ったコマンド、サイズ、所要時間です。メールの内容はここには記録されず、アプリを閉じると何も残りません。",
    "Copy activity": "記録をコピー",
    "Check again": "再確認",
    "just now": "たった今",
    "Working": "正常",
    "Working, with something to look at": "動作中 (確認事項あり)",
    "Not working": "動作していません",
    "Checking…": "確認中…",
    "Not in use": "未使用",
    "Signing in…": "サインイン中…",
    "Couldn't sign in: {why}": "サインインできませんでした: {why}",
    "Signed in to Gmail in {time}": "Gmail に {time} でサインイン",
    "Last sync failed: {why}": "前回の同期に失敗: {why}",
    "watching the inbox live": "受信トレイをリアルタイムで監視中",
    "not watching live, checking on a timer": "リアルタイム監視なし (定期的に確認)",
    "synced {when}": "最終同期: {when}",
    "{n} message listed": "{n} 件のメッセージを一覧化",
    "{n} messages listed": "{n} 件のメッセージを一覧化",
    "{n} change waiting to reach Gmail": "Gmail への反映待ちの変更 {n} 件",
    "{n} changes waiting to reach Gmail": "Gmail への反映待ちの変更 {n} 件",
    "No accounts yet.": "アカウントがまだありません。",
    "No API key yet, so the assistant is off.": "API キーが未設定のため、アシスタントはオフです。",
    "Asking the model for one word…": "モデルに一言だけ問い合わせ中…",
    "The model answered in {time}": "モデルが {time} で応答",
    "last answered {when}": "最後の応答: {when}",
    "last request failed {when}": "最後のリクエストは失敗: {when}",
    "an assistant": "アシスタント",
    "{n} request": "リクエスト {n} 件",
    "{n} requests": "リクエスト {n} 件",
    "last heard from {when}": "最後の通信: {when}",
    "Nothing has connected since Unibox started.": "Unibox の起動後、接続はありません。",
    "The helper for Claude Desktop is installed.": "Claude Desktop 用のヘルパーはインストール済みです。",
    "Mail": "メール",
    "Search index": "検索インデックス",
    "Disk": "ディスク",
    "local": "ローカル",
    "Local data": "ローカルデータ",
    "None connected": "接続なし",
    "Full text of all mail": "すべてのメールの全文",
    "Messages": "メッセージ",
    "Window cache": "ウィンドウのキャッシュ",
    "Message list": "メッセージ一覧",
    "Other": "その他",
    "Remove archived…": "アーカイブ分を削除…",
    "Removing…": "削除中…",
    "Remove the downloaded copies of conversations that are no longer in the inbox. They stay in Gmail.": "受信トレイにないスレッドのダウンロード済みデータを削除します。Gmail には残ります。",
    "Remove the downloaded copies of {n} messages in conversations that are no longer in the inbox ({size})?\n\nThey stay in Gmail and in the list and search here, and are downloaded again when you open them.": "受信トレイにないスレッドのメッセージ {n} 件のダウンロード済みデータ ({size}) を削除しますか?\n\nGmail にも、ここの一覧と検索にも残り、開くと再度ダウンロードされます。",
    "Removed the downloaded copy of {n} message.": "メッセージ {n} 件のダウンロード済みデータを削除しました。",
    "Removed the downloaded copies of {n} messages.": "メッセージ {n} 件のダウンロード済みデータを削除しました。",
    "Clear": "消去",
    "Clearing…": "消去中…",
    "Delete the pictures and pages the window has kept from the web. They are fetched again when needed.": "ウィンドウがウェブから保存した画像やページを削除します。必要になれば再取得されます。",
    "Measuring…": "計測中…",
    "{size} in all": "合計 {size}",
    "search index ready": "検索インデックス準備完了",
    "search index being built": "検索インデックスを作成中",
    "Nothing yet.": "まだ何もありません。",
    "Copied.": "コピーしました。",
    "Couldn't copy.": "コピーできませんでした。",
    "Subscriptions: every sender you can unsubscribe from (g then s)": "配信登録: 登録解除できる送信者の一覧 (g の後に s)",
    "Choose the language and the app's color": "言語とアプリの色を選ぶ",
    "Listing older mail… back to {month}": "古いメールを一覧に追加中… {month}まで",
    "Building the search index…": "検索インデックスを作成中…",

    // ---- the bar above the list
    "Search": "検索",
    "Search  ( / )": "検索  ( / )",
    "Inbox": "受信トレイ",
    "All mail": "すべてのメール",
    "Looking in all mail, archived and sent too. Click for just the inbox (g then i)":
      "アーカイブ済み・送信済みを含むすべてのメールを表示中。クリックで受信トレイのみ (g の後に i)",
    "Looking in the inbox. Click for all mail, archived and sent too (g then a)":
      "受信トレイを表示中。クリックでアーカイブ済み・送信済みを含むすべてのメール (g の後に a)",
    "Remove {what}": "{what} を外す",
    "Unread": "未読",
    "People": "人",
    "Automated": "自動送信",
    "Only unread conversations (f, or type is:unread)": "未読のスレッドのみ (f、または is:unread と入力)",
    "Only mail that a person wrote (Shift+F steps through, or type from:people)":
      "人が書いたメールのみ (Shift+F で切り替え、または from:people と入力)",
    "Only mail that systems sent: newsletters, receipts, notifications (Shift+F, or type from:automated)":
      "システムが送ったメールのみ: メールマガジン、領収書、通知 (Shift+F、または from:automated と入力)",
    "Who wrote it: one or neither": "差出人の種類: どちらか一方、または指定なし",
    "Suggestions": "候補",
    "Only unread": "未読のみ",
    "Only what people wrote": "人が書いたもののみ",
    "Only newsletters, receipts, notices": "メールマガジン、領収書、通知のみ",
    "Look in all mail": "すべてのメールから探す",
    "Look in the inbox": "受信トレイから探す",
    "Senders": "送信者",
    "↑↓ choose   Tab take   Esc close": "↑↓ 選択   Tab 確定   Esc 閉じる",
    "{n} message": "{n} 件",
    "{n} messages": "{n} 件",
    "Sync": "同期",
    "Syncing: checking Gmail for new mail and changes": "同期中: Gmail の新着と変更を確認しています",
    "Connected to Gmail: new mail shows up as it arrives. Click to check now.":
      "Gmail に接続中: 新着メールは届き次第表示されます。クリックで今すぐ確認。",
    "Next automatic check in {time}. Click to check now.": "次の自動確認まで {time}。クリックで今すぐ確認。",
    "Assistant": "アシスタント",
    "Assistant (Ctrl+K)": "アシスタント (Ctrl+K)",
    "Keyboard shortcuts": "キーボード ショートカット",
    "Keyboard shortcuts (?)": "キーボード ショートカット (?)",

    // ---- the list
    ", all mail": "、すべてのメール",
    "unread only": "未読のみ",
    "from people": "人から",
    "automated only": "自動送信のみ",
    "from {sender}": "差出人 {sender}",
    "{n} conversation": "{n} 件のスレッド",
    "{n} conversations": "{n} 件のスレッド",
    "{n} unread": "未読 {n} 件",
    "{n} selected": "{n} 件を選択中",
    "still listing older mail…": "古いメールを追加中…",
    "Back to the inbox": "受信トレイに戻る",
    "Add a Gmail account to get started.": "Gmail アカウントを追加して始めましょう。",
    "Nothing in all mail matches.": "すべてのメールに一致するものはありません。",
    "Nothing in the inbox matches.": "受信トレイに一致するものはありません。",
    "Fetching your inbox…": "受信トレイを取得中…",
    "Inbox zero.": "受信トレイは空です。",
    "A person wrote this": "人が書いたメール",
    "Archived": "アーカイブ済み",
    "(unknown)": "(不明)",
    "Drag to resize the list. Double-click to reset.": "ドラッグで一覧の幅を変更。ダブルクリックで元に戻します。",
    "Select a conversation to read it here.": "スレッドを選ぶとここに表示されます。",

    // ---- the conversation
    "Archive": "アーカイブ",
    "Archive (e)": "アーカイブ (e)",
    "Move to inbox": "受信トレイに移動",
    "Put this conversation back in the inbox": "このスレッドを受信トレイに戻す",
    "Mark unread": "未読にする",
    "Mark as unread (Shift+U)": "未読にする (Shift+U)",
    "Star": "スター",
    "Star (s)": "スター (s)",
    "★ Starred": "★ スター付き",
    "Expand all": "すべて展開",
    "Collapse all": "すべて折りたたむ",
    "Close every message but the last (:)": "最後のメッセージ以外を閉じる (:)",
    "Open every message in this conversation (;)": "このスレッドのメッセージをすべて開く (;)",
    "Reply in Gmail": "Gmail で返信",
    "Open this conversation in Gmail to reply (r)": "このスレッドを Gmail で開いて返信 (r)",
    "Open in window": "ウィンドウで開く",
    "Open this conversation in its own window (Shift+Enter, or Ctrl+click it in the list)":
      "このスレッドを別ウィンドウで開く (Shift+Enter、または一覧で Ctrl+クリック)",
    "Click for details.": "クリックで詳細を表示。",
    "View as plain text, or see the original message": "テキスト形式で表示、または元のメッセージを表示",
    "to {people}": "宛先: {people}",
    "via the group {group}": "グループ {group} 経由",
    "it was sent to": "(宛先のグループ)",
    "replies go to {address}": "返信先: {address}",
    "The sender's name shows {shown}, but this was sent from {actual}.":
      "差出人名には {shown} とありますが、実際の送信元は {actual} です。",
    "This message doesn't include one": "このメッセージには含まれていません",
    "Formatted": "書式付き",
    "Plain text": "テキスト形式",
    "Original message": "元のメッセージ",
    "Archive everything from {sender}…": "{sender} からのメールをすべてアーカイブ…",
    "Archive every inbox conversation with mail from {address}": "{address} からのメールを含む受信トレイのスレッドをすべてアーカイブ",
    "Show pictures": "画像を表示",
    "Stop showing this sender's pictures": "この送信者の画像を表示しない",
    "Sender details…": "送信者の詳細…",
    "Loading message…": "メッセージを読み込み中…",
    "Show": "表示",
    "Always from {sender}": "{sender} は常に表示",
    "Always show pictures in mail from {address}": "{address} からのメールの画像を常に表示",
    "Pictures hidden: Gmail couldn't verify this sender.": "画像を非表示にしています: Gmail はこの送信者を確認できませんでした。",
    "Pictures from {sender} will always be shown.": "{sender} からの画像を常に表示します。",
    "Pictures from {sender} are hidden again.": "{sender} からの画像を再び非表示にします。",
    "Says {shown} but goes to {real}": "表示は {shown} ですが、リンク先は {real} です",
    "Email {address}": "{address} にメール",

    // ---- attachments
    "Save": "保存",
    "Save to Downloads": "ダウンロードに保存",
    "Saved to {path}": "{path} に保存しました",
    "Loading…": "読み込み中…",
    "Couldn't load this file.": "このファイルを読み込めませんでした。",
    "No preview for this kind of file. Use Save to put it in Downloads.":
      "この種類のファイルはプレビューできません。「保存」でダウンロードに保存できます。",
    "{i} of {n}": "{i} / {n}",
    "Previous attachment (←)": "前の添付ファイル (←)",
    "Next attachment (→)": "次の添付ファイル (→)",
    "Close": "閉じる",
    "Close (Esc)": "閉じる (Esc)",

    // ---- unsubscribe and subscriptions
    "Unsubscribe": "登録解除",
    "Unsubscribed": "登録解除済み",
    "Unsubscribing…": "登録解除中…",
    "You asked this sender to stop.": "この送信者に配信停止を依頼しました。",
    "Tell {sender} to stop sending these. Nothing opens; the request goes straight to the sender.":
      "{sender} に配信停止を依頼します。何も開かず、依頼は送信者に直接送られます。",
    "Open this sender's unsubscribe page in your browser.": "この送信者の登録解除ページをブラウザーで開きます。",
    "Open a ready-made unsubscribe email in Gmail, for you to send.": "登録解除メールを Gmail で作成します。送信はご自身で行ってください。",
    "Unsubscribed from {sender}.": "{sender} の登録を解除しました。",
    "Couldn't unsubscribe: {why}": "登録解除できませんでした: {why}",
    "Opened the sender's unsubscribe page in your browser.": "送信者の登録解除ページをブラウザーで開きました。",
    "Opened an unsubscribe email in Gmail. Send it to leave the list.": "登録解除メールを Gmail で開きました。送信すると解除されます。",
    "Senders you can unsubscribe from, busiest first. After unsubscribing it can take a few days for mail to stop.":
      "登録解除できる送信者を、配信の多い順に表示します。解除後、メールが止まるまで数日かかることがあります。",
    "Looking…": "確認中…",
    "No senders with an unsubscribe link yet.": "登録解除リンクのある送信者はまだありません。",
    "Sender": "送信者",
    "Last 3 months": "過去 3 か月",
    "In all": "合計",
    "In inbox": "受信トレイ",
    "Show this sender's mail": "この送信者のメールを表示",
    "Archive the {n} in the inbox from this sender": "この送信者の受信トレイ内の {n} 件をアーカイブ",

    // ---- invitations and bookings
    "{date}, all day": "{date} 終日",
    "Invitation": "招待",
    "Canceled": "キャンセル済み",
    "Cancelled": "キャンセル済み",
    "Reply to an invitation": "招待への返信",
    "Event": "予定",
    "(no title)": "(タイトルなし)",
    ", and {n} more": "、他 {n} 人",
    " (your time; set in {zone})": " (現地時間に換算。元のタイムゾーン: {zone})",
    "When": "日時",
    ", repeating": "、繰り返し",
    "Where": "場所",
    "Organizer": "主催者",
    "Invited": "招待者",
    "Respond in Gmail": "Gmail で回答",
    "Open this conversation in Gmail, where you can accept or decline": "このスレッドを Gmail で開き、出欠を回答します",
    "Departs": "出発",
    "Arrives": "到着",
    "From": "差出人",
    "To": "宛先",
    "flight|From": "出発地",
    "flight|To": "到着地",
    "order|From": "販売元",
    "parcel|To": "お届け先",
    "Passenger": "搭乗者",
    "Check-in": "チェックイン",
    "Check-out": "チェックアウト",
    "Address": "住所",
    "Party": "人数",
    "Pick-up": "受け取り",
    "At": "場所",
    "Drop-off": "返却",
    "Until": "終了",
    "Status": "状況",
    "Total": "合計",
    "Carrier": "配送業者",
    "Tracking number": "追跡番号",
    "Expected": "お届け予定",
    "Booking reference": "予約番号",
    "Track": "追跡",
    "View": "表示",
    "Open": "開く",
    "Manage booking": "予約を管理",
    "Cancel booking": "予約をキャンセル",
    "Trip": "乗車予約",
    "Stay": "宿泊",
    "Table booking": "レストランの予約",
    "Car hire": "レンタカー",
    "Event booking": "イベントの予約",
    "Delivery": "配送",
    "Flight {rest}": "フライト {rest}",
    "Order {rest}": "注文 {rest}",

    // ---- is this sender genuine?
    "Is this sender genuine?": "この送信者は本物ですか?",
    "Passed": "合格",
    "Failed": "不合格",
    "Failed (soft)": "不合格 (ソフト)",
    "Not set up by the sender": "送信者が設定していません",
    "Inconclusive": "判定不能",
    "Couldn't be checked": "確認できませんでした",
    "Sender's record is broken": "送信者の設定に誤りがあります",
    "Rejected by policy": "ポリシーにより拒否",
    "No result": "結果なし",
    "None": "なし",
    "Gmail recorded no check for this message. That is normal for mail you sent yourself.":
      "Gmail はこのメッセージの確認結果を記録していません。ご自身が送信したメールでは通常のことです。",
    "Written by": "書いたのは",
    "A system, probably: it {why}.": "おそらくシステム: {why}。",
    "A person, probably: it {why}.": "おそらく人: {why}。",
    "Sent through": "経由",
    "the Google Group {group}": "Google グループ {group}",
    ". The checks below are the ones the group made on the sender.": "。以下はグループが送信者に対して行った確認結果です。",
    "Replies go to": "返信先",
    "Name claims": "名前の表示",
    "the domain asks for failing mail to be delivered anyway": "ドメインは、不合格のメールもそのまま配信するよう求めています",
    "the domain asks for failing mail to be treated as spam": "ドメインは、不合格のメールを迷惑メールとして扱うよう求めています",
    "the domain asks for failing mail to be refused": "ドメインは、不合格のメールを拒否するよう求めています",
    " (the domain publishes no policy)": " (ドメインはポリシーを公開していません)",
    "Forwarded": "転送",
    "Yes. It passed these checks where it first arrived.": "はい。最初に届いた場所でこれらの確認に合格しています。",
    "DKIM signature": "DKIM 署名",
    "{result}, signed by {domain}": "{result}、署名元 {domain}",
    ", for {domain}": "、対象 {domain}",
    "These are Gmail's own checks, recorded when the message arrived. A pass means the message really came from {domain}; it says nothing about whether that domain is one you should trust, so read it carefully.":
      "これらは Gmail 自身による確認で、メッセージの到着時に記録されたものです。合格は、メッセージが確かに {domain} から届いたことを意味しますが、そのドメインが信頼できるかどうかは示しません。ドメイン名をよくご確認ください。",
    "that domain": "そのドメイン",
    "The group's record": "グループの記録",
    "Gmail's record": "Gmail の記録",
    // (what the server says about why a person or a system wrote it)
    "says it was generated automatically": "自動生成されたと明記されています",
    "is marked as bulk mail": "一括送信メールとして送られています",
    "is from a mailing list that only sends": "配信専用のメーリング リストから届いています",
    "carries a bulk sender's tracking id": "一括送信サービスの追跡 ID が付いています",
    "comes from an address that isn't a person's": "個人のものではないアドレスから届いています",
    "was written to a Google Group": "Google グループ宛てに書かれています",
    "was posted to a discussion list": "メーリング リストに投稿されています",
    "was sent from this account": "このアカウントから送信されています",
    "was written in a mail program": "メール ソフトで書かれています",
    "is a reply in a conversation": "スレッド内の返信です",
    "shows no sign of a mail program or of being a reply": "メール ソフトで書かれた形跡も、返信である形跡もありません",

    // ---- things that happen
    "That didn't go through: {why}": "反映できませんでした: {why}",
    "Nothing to undo.": "元に戻す操作はありません。",
    "Undone.": "元に戻しました。",
    "Undo": "元に戻す",
    "Archived {n} message.": "メッセージ {n} 件をアーカイブしました。",
    "Archived {n} messages.": "メッセージ {n} 件をアーカイブしました。",
    "Archived {n} message from {sender}.": "{sender} からのメッセージ {n} 件をアーカイブしました。",
    "Archived {n} messages from {sender}.": "{sender} からのメッセージ {n} 件をアーカイブしました。",
    "Back to the previous chat": "前のチャットに戻る",
    "{n} sender": "送信者 {n} 件",
    "{n} senders": "送信者 {n} 件",
    "{n} still in your inbox": "受信トレイに {n} 件",
    "Archive all {n}": "{n} 件をすべてアーカイブ",
    "There is no earlier chat to bring back.": "元に戻せるチャットがありません。",
    "{n} conversations archived.": "{n} 件のスレッドをアーカイブしました。",
    "Moved to the inbox.": "受信トレイに移動しました。",
    "{n} conversations moved to the inbox.": "{n} 件のスレッドを受信トレイに移動しました。",
    "Marked read.": "既読にしました。",
    "{n} conversations marked read.": "{n} 件のスレッドを既読にしました。",
    "Marked unread.": "未読にしました。",
    "{n} conversations marked unread.": "{n} 件のスレッドを未読にしました。",
    "Starred.": "スターを付けました。",
    "{n} conversations starred.": "{n} 件のスレッドにスターを付けました。",
    "Unstarred.": "スターを外しました。",
    "{n} conversations unstarred.": "{n} 件のスレッドのスターを外しました。",
    "Back in the inbox.": "受信トレイに戻しました。",
    "{n} conversations back in the inbox.": "{n} 件のスレッドを受信トレイに戻しました。",
    "Couldn't undo: {why}": "元に戻せませんでした: {why}",
    "Archived.": "アーカイブしました。",
    "Archived {n} conversations from {sender}.": "{sender} からの {n} 件のスレッドをアーカイブしました。",
    "{n} conversations selected. Esc to clear.": "{n} 件のスレッドを選択しました。Esc で解除。",
    "Assistant archived {n} conversation.": "アシスタントが {n} 件のスレッドをアーカイブしました。",
    "Assistant archived {n} conversations.": "アシスタントが {n} 件のスレッドをアーカイブしました。",
    "Assistant moved {n} conversation to the inbox.": "アシスタントが {n} 件のスレッドを受信トレイに移動しました。",
    "Assistant moved {n} conversations to the inbox.": "アシスタントが {n} 件のスレッドを受信トレイに移動しました。",
    "Assistant marked {n} conversation read.": "アシスタントが {n} 件のスレッドを既読にしました。",
    "Assistant marked {n} conversations read.": "アシスタントが {n} 件のスレッドを既読にしました。",
    "Assistant marked {n} conversation unread.": "アシスタントが {n} 件のスレッドを未読にしました。",
    "Assistant marked {n} conversations unread.": "アシスタントが {n} 件のスレッドを未読にしました。",
    "Assistant starred {n} conversation.": "アシスタントが {n} 件のスレッドにスターを付けました。",
    "Assistant starred {n} conversations.": "アシスタントが {n} 件のスレッドにスターを付けました。",
    "Assistant unstarred {n} conversation.": "アシスタントが {n} 件のスレッドのスターを外しました。",
    "Assistant unstarred {n} conversations.": "アシスタントが {n} 件のスレッドのスターを外しました。",
    // (errors the server words for people)
    "This sender offers no one-click unsubscribe.": "この送信者はワンクリックの登録解除に対応していません。",
    "The unsubscribe address in this message doesn't look like a public website.": "このメッセージの登録解除アドレスは、公開されたウェブサイトではないようです。",
    "Couldn't reach the sender's site.": "送信者のサイトに接続できませんでした。",
    "The sender's site answered {code}.": "送信者のサイトが {code} を返しました。",
    "That message isn't stored yet.": "そのメッセージはまだ保存されていません。",
    "No such message.": "そのメッセージは見つかりません。",
    "Gmail didn't accept a change, so it has been undone: {why}": "Gmail が変更を受け付けなかったため、元に戻しました: {why}",

    // ---- adding an account
    "Change app password": "アプリ パスワードを変更",
    "Save password": "パスワードを保存",
    "Checking with Gmail…": "Gmail に確認中…",
    "App password changed. Syncing…": "アプリ パスワードを変更しました。同期中…",
    "Account added. Fetching the inbox…": "アカウントを追加しました。受信トレイを取得中…",
    "Gmail rejected that address and app password. Check both and try again.":
      "Gmail がそのアドレスとアプリ パスワードを受け付けませんでした。両方を確認して、もう一度お試しください。",
    "Email address": "メール アドレス",
    "App password": "アプリ パスワード",
    "16 letters": "16 文字",
    "Unibox signs in with an": "Unibox は",
    "app password": "アプリ パスワード",
    ", not your normal password. It is kept in the Windows Credential Manager on this PC.":
      "でサインインします (通常のパスワードではありません)。この PC の Windows 資格情報マネージャーに保存されます。",
    "Create one at": "作成はこちら:",
    "(the account needs 2-Step Verification on).": "(アカウントで 2 段階認証プロセスを有効にする必要があります)。",
    "Cancel": "キャンセル",

    // ---- the assistant
    "New chat": "新しいチャット",
    "Start a new conversation with the assistant": "アシスタントと新しい会話を始める",
    "Settings": "設定",
    "Open the assistant": "アシスタントを開く",
    "Ask about your mail, or tell it what to do…": "メールについて質問する、または指示する…",
    "Ask about your mail, or tell it what to do.": "メールについて質問したり、指示したりできます。",
    "“Find all my delivery emails”": "「配達関連のメールを全部探して」",
    "“Archive the ones from before this year”": "「去年までのものをアーカイブして」",
    "“What did they say about the refund?”": "「返金について何と言っていた?」",
    "“Draft a reply saying Saturday works”": "「土曜日で大丈夫と返信の下書きを作って」",
    "Working…": "処理中…",
    "Thinking…": "考え中…",
    "Looking through your mail…": "メールを確認中…",
    "Reading a conversation…": "スレッドを読んでいます…",
    "Archiving…": "アーカイブ中…",
    "Moving to the inbox…": "受信トレイに移動中…",
    "Marking as read…": "既読にしています…",
    "Marking as unread…": "未読にしています…",
    "Starring…": "スターを付けています…",
    "Removing stars…": "スターを外しています…",
    "Saving a draft in Gmail…": "Gmail に下書きを保存中…",
    "Showing “{title}”…": "「{title}」を表示中…",
    "Stop": "停止",
    "Send": "送信",
    "API key and model": "API キーとモデル",
    "The assistant runs on Claude. It needs an Anthropic API key, kept in the Windows Credential Manager.":
      "アシスタントは Claude で動作します。Anthropic の API キーが必要で、Windows 資格情報マネージャーに保存されます。",
    "When you ask it something, the mail it reads to answer is sent to the endpoint below.":
      "質問すると、回答のために読み取ったメールが下記のエンドポイントに送信されます。",
    "API key": "API キー",
    "Model": "モデル",
    "Endpoint": "エンドポイント",
    "(saved; leave empty to keep it)": "(保存済み。変更しない場合は空のまま)",

    // ---- appearance
    "Appearance": "表示",
    "Language": "言語",
    "Same as Windows": "Windows と同じ",
    "Color": "色",
    "The color used across Unibox. Its brightness is adjusted where needed so it stays readable in light and dark.":
      "Unibox 全体で使う色です。ライトとダークのどちらでも読みやすいよう、必要に応じて明るさを調整します。",
    "How vivid and how bright": "鮮やかさと明るさ",
    "Hue": "色相",
    "Color as #rrggbb": "色 (#rrggbb)",
    "Button": "ボタン",
    "Filter on": "フィルター有効",
    "Back to default": "既定に戻す",
    "Done": "完了",
    "Purple (the original)": "紫 (初期設定)",

    // ---- keyboard shortcuts
    "or": "または",
    "then": "の後に",
    "+click": "+クリック",
    "name": "名前",
    "Next / previous conversation": "次 / 前のスレッド",
    "Open conversation now": "スレッドを開く",
    "Open conversation in its own window": "スレッドを別ウィンドウで開く",
    "Clear the selection": "選択を解除",
    "Scroll the open conversation down / up": "開いているスレッドを下 / 上にスクロール",
    "Top / bottom of the open conversation": "開いているスレッドの先頭 / 末尾",
    "Next / previous message in a conversation": "スレッド内の次 / 前のメッセージ",
    "Expand / collapse every message in a conversation": "スレッドのメッセージをすべて展開 / 折りたたむ",
    "Star or unstar": "スターを付ける / 外す",
    "Mark read / unread": "既読 / 未読にする",
    "Undo the last archive, read/unread or star": "直前のアーカイブ、既読/未読、スターを元に戻す",
    "Select conversation (actions then apply to all selected)": "スレッドを選択 (以降の操作は選択したすべてに適用)",
    "Select all / none": "すべて選択 / 選択解除",
    "Reply (opens the conversation in Gmail)": "返信 (スレッドを Gmail で開く)",
    "Show only unread conversations (again to show all)": "未読のスレッドのみ表示 (もう一度押すと解除)",
    "Step through no filter / only people / only automated": "フィルターなし / 人のみ / 自動送信のみ を切り替え",
    "The same filters, typed in the search field": "同じフィルターを検索欄に入力",
    "Subscriptions: senders you can unsubscribe from": "配信登録: 登録解除できる送信者",
    "All Mail: archived and sent mail too": "すべてのメール: アーカイブ済み・送信済みを含む",
    "Back to all inboxes, clearing any search": "検索を解除して、すべての受信トレイに戻る",
    "Show only that account (": "そのアカウントのみ表示 (",
    "for all)": "ですべて)",
  },
};

/** The text in the language in use, with {names} filled in from `vars`. */
function tx(text, vars) {
  // ("flight|From" is "From" where it means a flight's origin: the part before the bar only
  // tells translations apart, and is dropped when there is none)
  let s = MESSAGES[LOCALE]?.[text] ?? text.replace(/^[a-z]+\|/, "");
  if (vars) s = s.replace(/\{(\w+)\}/g, (whole, k) => (k in vars ? vars[k] : whole));
  return s;
}

/** A number the way the language writes it. */
const fmtNum = n => Number(n).toLocaleString(LOCALE);

/** Text that depends on a count: `one` for exactly one (where the language has a singular), else `many`. */
function txn(n, one, many, vars) {
  return tx(n === 1 ? one : many, { n: fmtNum(n), ...vars });
}

// Sentences from the server with something variable in them: [what it looks like, the
// source text to translate with the captured pieces].
const SERVER_PATTERNS = [
  [/^Gmail confirmed this really came from (.+)\.$/, "verified", m => ({ sender: m[1] })],
  [/^This really came from (.+)\. It was checked before being forwarded to this account\.$/, "forwarded", m => ({ sender: m[1] })],
  [/^Gmail could not confirm this came from (.+)\. It may be forged\.$/, "failed", m => ({ sender: m[1] })],
  [/^Nothing proves this came from (.+?)\. It was sent through (.+)\.$/, "unverified-via", m => ({ sender: m[1], via: m[2] })],
  [/^Nothing proves this came from (.+)\.$/, "unverified", m => ({ sender: m[1] })],
];
const SERVER_TEXT = {
  "ja-JP": {
    "verified": "Gmail は、これが確かに {sender} から届いたことを確認しました。",
    "forwarded": "これは確かに {sender} から届いています。このアカウントに転送される前に確認されました。",
    "failed": "Gmail は、これが {sender} から届いたことを確認できませんでした。偽装の可能性があります。",
    "unverified-via": "これが {sender} から届いたことを示す証拠はありません。{via} 経由で送信されています。",
    "unverified": "これが {sender} から届いたことを示す証拠はありません。",
    "the sender": "送信者",
  },
};

/** Something the server said, in the language in use. Unknown text comes back as it was. */
function txServer(text) {
  if (!text) return text;
  const table = MESSAGES[LOCALE] || {};
  if (text in table) return table[text];
  const said = SERVER_TEXT[LOCALE];
  if (said) {
    for (const [shape, key, vars] of SERVER_PATTERNS) {
      const m = shape.exec(text);
      if (!m) continue;
      const v = vars(m);
      if (v.sender === "the sender") v.sender = said["the sender"];
      return said[key].replace(/\{(\w+)\}/g, (_, k) => v[k]);
    }
  }
  let m;
  if ((m = /^Listing older mail… back to (\w+) (\d{4})$/.exec(text))) {
    const when = new Date(`${m[1]} 1, ${m[2]}`);
    const month = isNaN(when) ? `${m[1]} ${m[2]}` : when.toLocaleDateString(LOCALE, { year: "numeric", month: "long" });
    return tx("Listing older mail… back to {month}", { month });
  }
  if ((m = /^Showing “(.*)”…$/.exec(text))) return tx("Showing “{title}”…", { title: m[1] });
  if ((m = /^(Flight|Order) (.+)$/.exec(text))) return tx(m[1] + " {rest}", { rest: m[2] });
  if ((m = /^The sender's site answered (\d+)\.$/.exec(text))) return tx("The sender's site answered {code}.", { code: m[1] });
  if ((m = /^Gmail didn't accept a change, so it has been undone: (.*)$/s.exec(text)))
    return tx("Gmail didn't accept a change, so it has been undone: {why}", { why: m[1] });
  return text;
}

/** Put the page's own fixed text (index.html) into the language in use: the words between
 *  tags, and tooltips, placeholders and labels for screen readers. */
function translatePage(root = document.body) {
  if (!MESSAGES[LOCALE]) return;
  const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const raw = n.nodeValue, key = raw.replace(/\s+/g, " ").trim();
    if (!key || n.parentElement.closest("script, style, svg")) continue;
    const said = MESSAGES[LOCALE][key];
    // keep the spaces around a piece of a sentence, so it still sits right among its neighbours
    if (said != null) n.nodeValue = raw.match(/^\s*/)[0] + said + raw.match(/\s*$/)[0];
  }
  for (const node of root.querySelectorAll("[title], [placeholder], [aria-label]")) {
    for (const attr of ["title", "placeholder", "aria-label"]) {
      const said = MESSAGES[LOCALE][node.getAttribute(attr)];
      if (said != null) node.setAttribute(attr, said);
    }
  }
}
