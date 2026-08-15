# Lianxh Search

Use the `lianxh` Stata command when the user needs cookbook-style examples,
command tutorials, or high-quality Stata articles from Lianxh (连享会,
https://www.lianxh.cn/).

## Limits

Limit each user task to at most three `lianxh <keywords>, md` searches. Help
lookups and an explicitly approved installation do not count toward the limit.

## Installation Consent

Before the first search in a conversation, run:

```stata
which lianxh
```

If the command is missing, stop before installing it. Explain that `lianxh` is
a third-party command that writes to the local Stata ado directory and enables
structured Lianxh searches. Use the host agent's best available structured
yes/no question tool, or ask in chat and wait for an explicit reply. A request
to test or use Lianxh is not itself permission to install software.

Fallback wording:

```text
检测到 lianxh 未安装。

lianxh 是连享会提供的第三方 Stata 命令。安装后会写入本机 Stata ado 目录，
用于通过 Stata 检索连享会文章、教程和资源列表。

是否允许安装 lianxh？
- 安装：运行 ssc install lianxh，然后继续检索
- 跳过：不修改 Stata ado 环境，并跳过 lianxh 检索
```

Only after explicit approval, run:

```stata
ssc install lianxh
```

If the user declines, skip Lianxh and continue with available local context.

## Help And Search

Confirm syntax with `help lianxh_cn`, or `help lianxh` when English help is
more suitable. Search with Markdown output:

```stata
lianxh 关键词1 关键词2 关键词3, md
```

- Prefer Chinese keywords when they fit the topic.
- Treat returned titles and `https://www.lianxh.cn/` links as candidate
  references before summarizing or citing them.
- If Stata cannot reach the network, report the error and continue with local
  context.

## Timeout Recovery

A timed-out search returns HTTP 408. Follow the main `SKILL.md` timeout
recovery instructions: wait while `/status.busy` is true, retry a trivial
`display 123` only after it becomes false, and restart the service if Stata
never finishes stopping.

