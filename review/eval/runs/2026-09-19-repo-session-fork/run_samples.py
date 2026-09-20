import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time
import tomllib

repo = Path('/home/rimuru/Projects/Code/for_fun/TeamAgents')
root = Path('/tmp/teamagents-repo-fork-20260919')
source_task = repo / 'review/eval/tasks/repo-session-fork'
harness = root / 'harness'
task = harness / 'review/eval/tasks/repo-session-fork'
shutil.copytree(source_task, task)
shutil.copy2(repo / 'review/eval/run.sh', harness / 'review/eval/run.sh')
(harness / 'engine').symlink_to(repo / 'engine', target_is_directory=True)
(harness / 'core').symlink_to(repo / 'core', target_is_directory=True)
(root / 'bin').mkdir()
shutil.copy2(repo / 'engine/target/debug/teamagents', root / 'bin/teamagents')
(root / 'config/teamagents').mkdir(parents=True)
shutil.copy2(Path('/tmp/teamagents-ledger-comparison-20260919/teamagents/config/teamagents/config.toml'), root / 'config/teamagents/config.toml')
profile = tomllib.loads((root / 'config/teamagents/config.toml').read_text())['models']['leader_main']
assert profile['context_window'] == 1_000_000
assert profile['generation_options']['reasoning_effort'] == 'high'
assert len(os.environ[profile['api_key_env']]) >= 8
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
production = {}
for crate in ['core','engine','tui']:
    for p in sorted((repo / crate / 'src').rglob('*.rs')):
        production[str(p.relative_to(repo))] = sha(p)
    for name in ['Cargo.toml','Cargo.lock']:
        p=repo / crate / name
        production[str(p.relative_to(repo))]=sha(p)
manifest = {
    'task':'repo-session-fork', 'date':'2026-09-19',
    'fixture_revision':tomllib.loads((task / 'fixture-source.toml').read_text())['revision'],
    'fixture_summary':json.loads((root / 'fixture-summary.json').read_text()),
    'model':profile['model'], 'provider':profile['provider'], 'protocol':profile['protocol'],
    'native_context_window':profile['context_window'],
    'context_window_source':'用户确认的 DeepSeek Flash 原生 1M，docs/DECISIONS.md D-36',
    'reasoning_effort':'high','request_timeout_seconds':profile['timeout'],'max_retries':profile['max_retries'],
    'task_timeout_seconds':1200, 'samples':3,'execution':'three sequential fresh independent sessions',
    'binary_sha256':sha(root / 'bin/teamagents'),
    'configuration_sha256':sha(root / 'config/teamagents/config.toml'),
    'configuration_source':'same isolated model/provider settings as 2026-09-19 ledger comparison; credentials via environment',
    'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),
    'workspace_dirty':bool(subprocess.check_output(['git','status','--porcelain'],cwd=repo)),
    'task_sha256':{str(p.relative_to(task)):sha(p) for p in sorted(task.rglob('*')) if p.is_file()},
    'production_sha256':production,
    'grader_sha256':sha(repo / 'engine/tests/eval_grader.rs'),
    'runner_sha256':sha(harness / 'review/eval/run.sh'),
    'runner_contract_sha256':sha(repo / 'review/eval/check-runner.sh'),
    'rustc':subprocess.check_output(['rustc','--version'],text=True).strip(),
    'started_unix':time.time(),
}
(root / 'manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n')
for sample in range(1,4):
    assert sha(root / 'bin/teamagents') == manifest['binary_sha256']
    assert sha(repo / 'engine/tests/eval_grader.rs') == manifest['grader_sha256']
    assert all(sha(task/p)==s for p,s in manifest['task_sha256'].items())
    run = root / f'sample-{sample}'
    run.mkdir()
    env = dict(os.environ, XDG_CONFIG_HOME=str(root / 'config'))
    started=time.time()
    print(json.dumps({'sample':sample,'state':'started','unix':started}),flush=True)
    with (run/'runner.log').open('w') as out, (run/'driver.stderr').open('w') as err:
        process = subprocess.Popen(['bash', str(harness/'review/eval/run.sh'), '--bin',str(root/'bin/teamagents'),
            '--only','repo-session-fork','--timeout','1200','--out',str(run/'run')],
            cwd=repo,env=env,stdout=out,stderr=err,start_new_session=True)
        try:
            code=process.wait(timeout=1950)
            outer_timeout=False
        except subprocess.TimeoutExpired:
            outer_timeout=True
            os.killpg(process.pid,signal.SIGTERM)
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid,signal.SIGKILL)
                process.wait()
            code=124
    result={'sample':sample,'exit_code':code,'outer_timeout':outer_timeout,
        'duration_seconds':time.time()-started,'started_unix':started,'finished_unix':time.time()}
    (run/'exit.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result),flush=True)
(root/'samples-finished.json').write_text(json.dumps({'finished_unix':time.time(),'samples':3})+'\n')
