-- 監査ログの保存期間（11-permissions-and-audit.md「保護と保存期間」、第三者の確認の R33）。
-- 普段用のロールは読み書きできない。普段用のロールを奪われても、保存期間を縮めて記録を消せないようにする。
CREATE TABLE yomi.audit_retention (
    only_row boolean PRIMARY KEY DEFAULT true CHECK (only_row),
    days     integer NOT NULL CHECK (days >= 30)
);
INSERT INTO yomi.audit_retention (days) VALUES (365);
