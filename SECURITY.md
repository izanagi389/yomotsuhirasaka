# Security policy / セキュリティの方針

[English](#english) | [日本語](#日本語)

## English

### Reporting a vulnerability

Please **do not report vulnerabilities in public issues, discussions or pull requests.**

Report them privately with GitHub's private vulnerability reporting: open the repository's **Security** tab and choose **Report a vulnerability**. Only the maintainers can read the report.

Please include:

- the affected component (server, admin UI, CLI, Nuxt module, official Docker Compose) and version or commit
- the steps to reproduce, and what an attacker can do
- whether you have shared it with anyone else

Please do not include real secrets (passwords, API keys, tokens, encryption keys) or other people's personal data in the report. Test only against instances you run yourself.

We will confirm the report, investigate it, and agree with you on when to publish it. Once a fix is released, we publish a GitHub security advisory. Tell us if you would like to be credited.

### Supported versions

Security fixes are released for the **latest minor version only**. Please upgrade to the latest release to receive them.

| Version | Security fixes |
|---|---|
| Latest minor version | Yes |
| Older versions | No |

Before v1.0 there are no releases, and fixes go only to the `main` branch.

### Scope

In scope: the CMS server, the admin UI, the CLI, the official Nuxt module and the shared packages in this repository, and the official Docker Compose in `deploy/`.

Out of scope: sites built with the CMS, your own templates and scripts, third-party LLM providers, and attacks that flood the network itself (L3/L4 DDoS), which need protection at the CDN or hosting layer.

## 日本語

### 脆弱性の報告

**脆弱性は、公開の Issue・Discussions・プルリクエストに書かないでください。**

GitHub の非公開の脆弱性報告（Private vulnerability reporting）で知らせてください。リポジトリの **Security** タブを開き、**Report a vulnerability** を選びます。報告は保守する人だけが読めます。

報告には、次のことを書いてください。

- 影響を受ける部分（サーバー、管理画面、CLI、Nuxt モジュール、公式の Docker Compose）と、版またはコミット
- 再現の手順と、攻撃する人に何ができるか
- ほかの人に伝えたかどうか

報告には、本物の秘密の値（パスワード、API キー、トークン、暗号化の鍵）や、ほかの人の個人情報を入れないでください。試すのは、自分で動かしているインスタンスだけにしてください。

報告を受け取ったら確認し、調べて、いつ公表するかを報告した人と相談して決めます。修正を出したら、GitHub のセキュリティアドバイザリで公表します。名前を載せてよい場合は、そう伝えてください。

### 修正を出す版

セキュリティの修正は、**最新のマイナー版にだけ**出します。修正を受けるには、最新の版に上げてください。

| 版 | セキュリティの修正 |
|---|---|
| 最新のマイナー版 | 出す |
| それより古い版 | 出さない |

v1.0 の前はリリースがないので、修正は `main` ブランチにだけ入れます。

### 対象

対象: このリポジトリの CMS のサーバー、管理画面、CLI、公式の Nuxt モジュールと共有のパッケージ、`deploy/` の公式の Docker Compose。

対象外: CMS を使って作ったサイト、利用者が書いたテンプレートとスクリプト、外部の LLM の提供会社、回線そのものをあふれさせる攻撃（L3/L4 の DDoS。CDN やホスティングの側で備える）。
