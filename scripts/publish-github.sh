#!/usr/bin/env bash
# Публикация обезличенного зеркала репозитория в публичный GitHub (TheDukeArt/voicedo).
# Gitea остаётся основным репо: история локального репозитория не переписывается,
# фильтр применяется к временной копии и зеркалируется force-push'ом.
# Требует: git-filter-repo, gh (авторизованного под TheDukeArt).
set -euo pipefail

GH_REPO="TheDukeArt/voicedo"
GH_BRANCH="main"
PUB_NAME="TheDukeArt"
PUB_EMAIL="324849320+TheDukeArt@users.noreply.github.com"

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/voicedo-publish.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

echo "==> Клон для обезличивания: $REPO_DIR"
git clone --quiet "$REPO_DIR" "$WORK/voicedo"
cd "$WORK/voicedo"
git remote remove origin

git filter-repo --force --quiet \
  --name-callback "return b'$PUB_NAME'" \
  --email-callback "return b'$PUB_EMAIL'" \
  --refname-callback 'return b"refs/heads/' + (b'main' if refs == b'refs/heads/master' else refs[11:])'

BAD="$(git log --all --format='%ae %ce' | grep -vF "$PUB_EMAIL" | grep -c '@' || true)"
if [ "$BAD" != "0" ]; then
  echo "ОШИБКА: в истории остались сторонние почты ($BAD)" >&2
  git log --all --format='%ae %ce' | grep -vF "$PUB_EMAIL" | sort -u >&2
  exit 1
fi
echo "==> История обезличена ($(git rev-list --count HEAD) коммитов)"

gh repo view "$GH_REPO" --json name >/dev/null 2>&1 || {
  echo "==> Создаю $GH_REPO"
  gh repo create "$GH_REPO" --public \
    --description "VoiceDo — free, open-source push-to-talk dictation (Tauri + Svelte) with your own ASR provider. https://voicedo.app"
}

gh repo edit "$GH_REPO" \
  --homepage "https://voicedo.app" \
  --add-topic tauri --add-topic svelte --add-topic dictation \
  --add-topic speech-to-text --add-topic voice-input --add-topic rust

REMOTE="https://github.com/$GH_REPO.git"
echo "==> Зеркалирую main и теги (force)"
git push --force "$REMOTE" "HEAD:refs/heads/$GH_BRANCH"
git push --force "$REMOTE" --tags 2>/dev/null || echo "   (тегов нет или отклонены)"

echo "==> Готово: https://github.com/$GH_REPO"
