"""Generate version notes from Git history, including repositories without PRs."""
import re
import subprocess

GROUPS = [
    ('feat', '新增功能'), ('fix', '问题修复'), ('perf', '性能优化'),
    ('refactor', '代码重构'), ('ci', '构建与发布'), ('docs', '文档更新'),
    ('test', '测试'), ('chore', '其他变更'),
]


def git(repo, *arguments):
    return subprocess.check_output(['git', '-C', str(repo), *arguments], text=True).strip()


def generate_notes(repo, tag, commit, repository):
    """Use only commits included in the version; never include later repackaging changes."""
    previous = subprocess.run(
        ['git', '-C', str(repo), 'describe', '--tags', '--match', 'v[0-9]*', '--abbrev=0', commit + '^'],
        capture_output=True, text=True)
    previous_tag = previous.stdout.strip() if previous.returncode == 0 else None
    revision = previous_tag + '..' + commit if previous_tag else commit
    history = git(repo, 'log', '--no-merges', '--format=%H%x09%s', revision)
    groups = {key: [] for key, _ in GROUPS}
    url = 'https://github.com/' + repository
    for line in history.splitlines():
        sha, subject = line.split('\t', 1)
        kind = re.match(r'([a-z]+)(?:\([^)]*\))?!?:\s*', subject)
        group = kind.group(1) if kind else 'chore'
        group = {'build': 'ci', 'tests': 'test'}.get(group, group)
        if group not in groups:
            group = 'chore'
        # Commit subjects are source material, not Markdown instructions.
        title = re.sub(r'([\\`*_\[\]<>])', r'\\\1', subject)
        groups[group].append(f'- {title} ([{sha[:7]}]({url}/commit/{sha}))')
    notes = [f'## {tag}', '', '以下内容根据本版本的 Git 提交记录自动生成。', '']
    for key, heading in GROUPS:
        if groups[key]:
            notes += [f'### {heading}', '', *groups[key], '']
    history_url = f'{url}/compare/{previous_tag}...{tag}' if previous_tag else f'{url}/commits/{tag}'
    notes += [f'[完整变更记录]({history_url})', '']
    return '\n'.join(notes)
