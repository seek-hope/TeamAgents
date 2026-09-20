"""Recompute old-result visibility from completed trees; no model reasoning exported."""
from pathlib import Path
import collections
import json

ROOT=Path('/tmp/teamagents-repo-fork-20260919')

def hidden_outputs(messages):
    # Mirror the production L1 loop. The page receipts examined here are below
    # L0's 50k character cap, so their exact UTF-8 byte lengths are available.
    last_assistant=next((i for i in range(len(messages)-1,-1,-1) if messages[i].get('role')=='assistant'),0)
    budget=16000
    hidden=set()
    for i in range(last_assistant,-1,-1):
        m=messages[i]
        if m.get('role')!='tool':continue
        content=m.get('content','')
        size=len(content.encode())
        if len(content)>50000:
            # Even capped output cannot fit this budget; its exact hint size
            # cannot change the visibility decision or remaining zero budget.
            size=50001
        if budget>=size:budget-=size
        else:
            hidden.add(m.get('tool_call_id'))
            budget=0
    return hidden

for sample in range(1,4):
    run=ROOT/f'sample-{sample}'
    if not (run/'exit.json').exists():continue
    rows=[]
    repeats=[]
    repeated_file_reads=[]
    members={}
    for path in (run/'run/state').rglob('chat_tree.json'):
        for thread in json.loads(path.read_text()).values():
            members.setdefault(path.parent.name,[]).append([n['message'] for n in thread['nodes']])
    for path in (run/'run/state').rglob('turns/*.json'):
        members.setdefault(path.parent.parent.name,[]).append(json.loads(path.read_text())['history'])
    for member,histories in members.items():
        seen_calls=set()
        for messages in sorted(histories,key=len,reverse=True):
            previous={}
            file_pages={}
            tool_kinds={}
            readbacks={}
            steps=0
            for i,m in enumerate(messages):
                if m.get('role')!='assistant':continue
                steps+=1
                hidden=hidden_outputs(messages[:i])
                for call in m.get('tool_calls',[]):
                    name=call.get('function',{}).get('name')
                    tool_kinds[call['id']]=name
                    first_seen=call['id'] not in seen_calls
                    seen_calls.add(call['id'])
                    if name=='read_file':
                        a=json.loads(call['function']['arguments'])
                        key=(a.get('path'),a.get('offset',1),a.get('byte_offset'),a.get('limit',2000))
                        if key in file_pages and first_seen:
                            repeated_file_reads.append({
                                'member':member,'request_step':steps,
                                'path':key[0],'offset':key[1],'byte_offset':key[2],'limit':key[3],
                                'previous_call_id':file_pages[key],'call_id':call['id'],
                                'previous_receipt_hidden':file_pages[key] in hidden})
                        file_pages[key]=call['id']
                    if name!='read_history':continue
                    a=json.loads(call['function']['arguments'])
                    source=a.get('tool_call_id')
                    key=(source,a.get('offset',0),a.get('limit',12000))
                    depth=0
                    ancestor=source
                    seen=set()
                    while ancestor in readbacks and ancestor not in seen:
                        depth+=1;seen.add(ancestor);ancestor=readbacks[ancestor]
                    row={'member':member,'request_step':steps,'call_id':call['id'],
                         'source_id':source,'source_tool':tool_kinds.get(source),
                         'offset':key[1],'limit':key[2],'readback_ancestor_depth':depth,
                         'source_hidden_before_request':source in hidden,
                         'prior_identical_page_id':previous.get(key),
                         'prior_identical_page_hidden':previous[key] in hidden if key in previous else None}
                    if first_seen:
                        rows.append(row)
                        if key in previous:repeats.append(row)
                    previous[key]=call['id']
                    readbacks[call['id']]=source
    telemetry=[json.loads(line) for line in (run/'run/repo-session-fork.jsonl').read_text().splitlines() if line.strip()]
    actual_calls={r['call_id'] for r in telemetry if r.get('type')=='tool' and r.get('tool')=='read_history'}
    covered_calls={r['call_id'] for r in rows}
    report={'method':'从已保存对话树和回合检查点复算生产 L1 的 16,000 字节遮蔽循环，按调用 ID 去重；不是逐请求网络抓包，不导出模型推理内容',
            'telemetry_readback_calls':len(actual_calls),
            'readbacks_without_saved_request_context':sorted(actual_calls-covered_calls),
            'read_history_calls':len(rows),'recursive_readback_calls':sum(r['readback_ancestor_depth']>0 for r in rows),
            'identical_page_repeats':len(repeats),
            'repeats_with_previous_page_hidden':sum(r['prior_identical_page_hidden'] is True for r in repeats),
            'repeats_with_previous_page_visible':sum(r['prior_identical_page_hidden'] is False for r in repeats),
            'identical_file_page_repeats':len(repeated_file_reads),
            'file_repeats_with_previous_receipt_hidden':sum(r['previous_receipt_hidden'] for r in repeated_file_reads),
            'repeated_file_pages':repeated_file_reads,
            'calls':rows}
    (run/'readback-diagnostic.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    print(run.name,{k:v for k,v in report.items() if k not in ['method','calls','repeated_file_pages']})
