#!/usr/bin/env bash
# Later 七项(L1–L7)后端 E2E:各能力 curl 往返断言。
# 前置:report-server :8092 在跑;model-center :8099 已部署 cg_* 元数据(22 表)。
set -u
B=http://localhost:8092/api
pass=0; fail=0
ok(){ echo "✅ $1"; pass=$((pass+1)); }
no(){ echo "❌ $1 — $2"; fail=$((fail+1)); }
jq_get(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }

echo "=== L1 同一控制下企业合并(权益结合法) ==="
# CC_TEST:S 同控子(under_common_control=1),差额入资本公积不确认商誉。
curl -s -m8 -X POST $B/consol/schemes -H 'content-type: application/json' -d '{"schemeCode":"CC_TEST","name":"同一控制测试","standard":"CAS","groupCurrency":"CNY","investmentAccount":"1511","goodwillAccount":"1801","nciAccount":"4400","minorityPlAccount":"4900","ctaAccount":"4106","capitalReserveAccount":"4002"}' >/dev/null
curl -s -m8 -X POST $B/consol/group-accounts -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"CC_TEST","accountCode":"1001","name":"现金","accountType":"asset"},
 {"schemeCode":"CC_TEST","accountCode":"1511","name":"长期股权投资","accountType":"asset"},
 {"schemeCode":"CC_TEST","accountCode":"1801","name":"商誉","accountType":"asset"},
 {"schemeCode":"CC_TEST","accountCode":"4001","name":"实收资本","accountType":"equity"},
 {"schemeCode":"CC_TEST","accountCode":"4002","name":"资本公积","accountType":"equity"},
 {"schemeCode":"CC_TEST","accountCode":"4104","name":"未分配利润","accountType":"equity"},
 {"schemeCode":"CC_TEST","accountCode":"4400","name":"少数股东权益","accountType":"nci"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/scope -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"CC_TEST","periodCode":"2026-01","orgCode":"GRP","orgName":"集团","parentCode":"","consolMethod":"full","ownershipPct":1,"isLeaf":0,"levelNo":1},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","orgCode":"P","orgName":"母","parentCode":"GRP","consolMethod":"full","ownershipPct":1,"isLeaf":1,"levelNo":2},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","orgCode":"S","orgName":"同控子","parentCode":"GRP","consolMethod":"full","ownershipPct":0.8,"isLeaf":1,"levelNo":2,"investmentAmount":140,"underCommonControl":1}]}' >/dev/null
curl -s -m8 -X POST $B/consol/entity-balances -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"1001","amount":160},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"1511","amount":140},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4001","amount":-250},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4104","amount":-50},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"1001","amount":150},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"4001","amount":-100},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"4002","amount":-20},
 {"schemeCode":"CC_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"4104","amount":-30}]}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"CC_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/worksheet?scheme=CC_TEST&period=2026-01&node=GRP")
gw=$(echo "$r" | jq_get "next((x['consolidated'] for x in d['data']['rows'] if x['account_code']=='1801'),'0')")
bs=$(echo "$r" | jq_get "round(sum(float(x['consolidated']) for x in d['data']['rows']),4)")
[ "$(echo "$gw==0" | bc 2>/dev/null || python3 -c "print(1 if abs(float('$gw'))<0.005 else 0)")" = "1" ] && ok "L1 商誉=0(不确认商誉)" || no "L1 商誉" "gw=$gw"
[ "$bs" = "0.0" ] && ok "L1 BS 恒等=0" || no "L1 BS" "bs=$bs"

echo "=== L2 分步取得(原持股公允重估) ==="
curl -s -m8 -X POST $B/consol/group-accounts -H 'content-type: application/json' -d '{"items":[{"schemeCode":"CC_TEST","accountCode":"6111","name":"投资收益","accountType":"income"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/step-txn -H 'content-type: application/json' -d '{"items":[{"schemeCode":"CC_TEST","periodCode":"2026-01","nodeCode":"GRP","txnType":"step_acq","prevCarrying":100,"prevFairValue":130}]}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"CC_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/worksheet?scheme=CC_TEST&period=2026-01&node=GRP")
inc=$(echo "$r" | jq_get "next((x['adjust'] for x in d['data']['rows'] if x['account_code']=='6111'),'0')")
[ "$(python3 -c "print(1 if abs(float('$inc')+30)<0.005 else 0)")" = "1" ] && ok "L2 投资收益重估 adjust=-30" || no "L2 重估" "inc=$inc"

echo "=== L3 固定资产内部交易未实现利润 ==="
curl -s -m8 -X POST $B/consol/group-accounts -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"CC_TEST","accountCode":"1601","name":"固定资产","accountType":"asset"},
 {"schemeCode":"CC_TEST","accountCode":"1602","name":"累计折旧","accountType":"asset"},
 {"schemeCode":"CC_TEST","accountCode":"6301","name":"资产处置损益","accountType":"expense"},
 {"schemeCode":"CC_TEST","accountCode":"6602","name":"折旧费用","accountType":"expense"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/fa-profit -H 'content-type: application/json' -d '{"items":[{"schemeCode":"CC_TEST","periodCode":"2026-01","seller":"P","buyer":"S","unrealized":100,"depYears":5,"elapsedYears":2}]}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"CC_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/worksheet?scheme=CC_TEST&period=2026-01&node=GRP")
fa=$(echo "$r" | jq_get "next((x['elim'] for x in d['data']['rows'] if x['account_code']=='1601'),'0')")
dep=$(echo "$r" | jq_get "next((x['elim'] for x in d['data']['rows'] if x['account_code']=='1602'),'0')")
[ "$(python3 -c "print(1 if abs(float('$fa')+100)<0.005 else 0)")" = "1" ] && ok "L3 固定资产 elim=-100" || no "L3 固资" "fa=$fa"
[ "$(python3 -c "print(1 if abs(float('$dep')-40)<0.005 else 0)")" = "1" ] && ok "L3 折旧转回 +40" || no "L3 折旧" "dep=$dep"

echo "=== L4 内部股利抵销 ==="
curl -s -m8 -X POST $B/consol/elim-rules -H 'content-type: application/json' -d '{"items":[{"schemeCode":"CC_TEST","ruleCode":"R_DIV","elimType":"dividend","drAccount":"6111","crAccount":"4104","enabled":1}]}' >/dev/null
curl -s -m8 -X POST $B/consol/ic-matches -H 'content-type: application/json' -d '{"items":[{"schemeCode":"CC_TEST","periodCode":"2026-01","entityA":"P","entityB":"S","icType":"dividend","amount":40}]}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"CC_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/journal?scheme=CC_TEST&period=2026-01&node=GRP")
divn=$(echo "$r" | jq_get "sum(1 for e in d['data']['entries'] if e['elim_type']=='dividend')")
[ "$divn" = "2" ] && ok "L4 股利抵销凭证 2 行" || no "L4 股利" "divn=$divn"

echo "=== L5 现金流量表·工作底稿法(CAS_LEGAL 两期) ==="
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"CAS_LEGAL","period":"2026-06"}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"CAS_LEGAL","period":"2026-12"}' >/dev/null
curl -s -m10 -X POST $B/consol/cashflow/worksheet -H 'content-type: application/json' -d '{"scheme":"CAS_LEGAL","period":"2026-12"}' >/dev/null
r=$(curl -s -m8 "$B/consol/cash-flow?scheme=CAS_LEGAL&period=2026-12&node=CSCEC")
tie=$(echo "$r" | python3 -c "
import sys,json
d=json.load(sys.stdin)['data']
ws={x['item_code']:float(x['amount']) for x in d['rows'] if x['item_code'] in ('CF_OP','CF_INV','CF_FIN','CF_NET')}
s=ws.get('CF_OP',0)+ws.get('CF_INV',0)+ws.get('CF_FIN',0)
print(1 if abs(s-ws.get('CF_NET',999))<0.005 else 0)
" 2>/dev/null)
[ "$tie" = "1" ] && ok "L5 三活动净额=现金实际变动" || no "L5 现金流" "tie=$tie"

echo "=== L6 交叉持股·有效持股 ==="
curl -s -m8 -X POST $B/consol/shareholding -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"XH_TEST","periodCode":"2026-01","holder":"GRP","held":"A","pct":0.8,"isParent":1},
 {"schemeCode":"XH_TEST","periodCode":"2026-01","holder":"A","held":"B","pct":0.6},
 {"schemeCode":"XH_TEST","periodCode":"2026-01","holder":"B","held":"A","pct":0.1}]}' >/dev/null
r=$(curl -s -m8 "$B/consol/effective-ownership?scheme=XH_TEST&period=2026-01")
a=$(echo "$r" | jq_get "float(d['data']['effective']['A'])")
[ "$(python3 -c "print(1 if abs(float('$a')-0.851063)<0.001 else 0)")" = "1" ] && ok "L6 有效持股 A≈0.8511(收敛)" || no "L6 有效持股" "a=$a"

echo "=== L7 附注自动生成(GW_TEST) ==="
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"GW_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/notes?scheme=GW_TEST&period=2026-01")
gwc=$(echo "$r" | jq_get "float(d['data']['notes']['goodwill']['closingBalance'])")
[ "$(python3 -c "print(1 if abs(float('$gwc')-15)<0.005 else 0)")" = "1" ] && ok "L7 附注商誉期末=15" || no "L7 附注" "gwc=$gwc"

echo ""
echo "=== $pass passed / $fail failed ==="
[ "$fail" = "0" ] && exit 0 || exit 1
