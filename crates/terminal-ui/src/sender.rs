/// `Sender` 表示一条消息的发送者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    User,
    Assistant,
}
