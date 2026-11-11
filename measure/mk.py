import json,sys
src=json.load(open(sys.argv[1]))
out=[[n,net,f'echo "## {n}"; {c} 2>&1; echo "rc=$?"'] for n,net,c in src]
json.dump(out,open(sys.argv[2],'w'))
