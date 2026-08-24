#!/usr/bin/env bash
# 正交/择机项 O1–O6 后端 E2E:各能力 curl 往返断言(O7 是一键部署脚本,单独跑)。
# 前置:report-server :8092 在跑;model-center 已部署 cg_* 元数据(24 表,含 cg_net_investment_hedge/cg_goodwill_cgu)。
set -u
B=http://localhost:8092/api
pass=0; fail=0
ok(){ echo "✅ $1"; pass=$((pass+1)); }
no(){ echo "❌ $1 — $2"; fail=$((fail+1)); }
g(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }
near(){ python3 -c "print(1 if abs(float('$1')-($2))<0.005 else 0)" 2>/dev/null; }

echo "=== O1 净投资套期(外币):有效部分→CTA 储备对冲折算差额 ==="
curl -s -m8 -X POST $B/consol/schemes -H 'content-type: application/json' -d '{"schemeCode":"NIH_TEST","name":"净投资套期测试","standard":"CAS","groupCurrency":"CNY","investmentAccount":"1511","goodwillAccount":"1801","nciAccount":"4400","minorityPlAccount":"4900","ctaAccount":"4106","capitalReserveAccount":"4002"}' >/dev/null
curl -s -m8 -X POST $B/consol/group-accounts -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"NIH_TEST","accountCode":"1001","name":"货币资金","accountType":"asset"},
 {"schemeCode":"NIH_TEST","accountCode":"4001","name":"实收资本","accountType":"equity"},
 {"schemeCode":"NIH_TEST","accountCode":"4104","name":"未分配利润","accountType":"equity"},
 {"schemeCode":"NIH_TEST","accountCode":"4106","name":"外币折算差额","accountType":"equity"},
 {"schemeCode":"NIH_TEST","accountCode":"4107","name":"套期储备","accountType":"equity"},
 {"schemeCode":"NIH_TEST","accountCode":"4400","name":"少数股东权益","accountType":"nci"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/scope -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","orgCode":"GRP","orgName":"集团","parentCode":"","consolMethod":"full","ownershipPct":1,"isLeaf":0,"levelNo":1},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","orgCode":"P","orgName":"母","parentCode":"GRP","consolMethod":"full","ownershipPct":1,"isLeaf":1,"levelNo":2,"currency":"CNY"},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","orgCode":"F","orgName":"境外子","parentCode":"GRP","consolMethod":"full","ownershipPct":1,"isLeaf":1,"levelNo":2,"currency":"USD"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/entity-balances -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"1001","amount":300},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4001","amount":-250},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4104","amount":-50},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","entityCode":"F","accountCode":"1001","amount":200},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","entityCode":"F","accountCode":"4001","amount":-150},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","entityCode":"F","accountCode":"4104","amount":-50}]}' >/dev/null
curl -s -m8 -X POST $B/consol/fx-rates -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","fromCcy":"USD","toCcy":"CNY","rateType":"closing","rate":7.0},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","fromCcy":"USD","toCcy":"CNY","rateType":"average","rate":6.8},
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","fromCcy":"USD","toCcy":"CNY","rateType":"historical","rate":6.5}]}' >/dev/null
# 套期:GRP 节点,套期工具 4107,有效部分 60(从 CTA 重分类)。
curl -s -m8 -X POST $B/consol/net-investment-hedge -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"NIH_TEST","periodCode":"2026-01","nodeCode":"GRP","hedgeInstrumentAccount":"4107","effectiveAmount":60}]}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"NIH_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/worksheet?scheme=NIH_TEST&period=2026-01&node=GRP")
cta=$(echo "$r" | g "next((x['adjust'] for x in d['data']['rows'] if x['account_code']=='4106'),'0')")
hdg=$(echo "$r" | g "next((x['consolidated'] for x in d['data']['rows'] if x['account_code']=='4107'),'0')")
bs=$(echo "$r" | g "round(sum(float(x['consolidated']) for x in d['data']['rows']),4)")
[ "$(near "$cta" -60)" = "1" ] && ok "O1 CTA 重分类 adjust=-60" || no "O1 CTA" "cta=$cta"
[ "$(near "$hdg" 60)" = "1" ] && ok "O1 套期储备 consolidated=+60" || no "O1 套期储备" "hdg=$hdg"
[ "$bs" = "0.0" ] && ok "O1 BS 恒等=0" || no "O1 BS" "bs=$bs"

echo "=== O2 商誉减值测试(CGU 可收回金额 vs 账面) ==="
curl -s -m8 -X POST $B/consol/schemes -H 'content-type: application/json' -d '{"schemeCode":"O2_TEST","name":"商誉减值测试","standard":"CAS","groupCurrency":"CNY","investmentAccount":"1511","goodwillAccount":"1801","nciAccount":"4400","minorityPlAccount":"4900","ctaAccount":"4106","capitalReserveAccount":"4002"}' >/dev/null
curl -s -m8 -X POST $B/consol/group-accounts -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O2_TEST","accountCode":"1001","name":"货币资金","accountType":"asset"},
 {"schemeCode":"O2_TEST","accountCode":"1511","name":"长期股权投资","accountType":"asset"},
 {"schemeCode":"O2_TEST","accountCode":"1801","name":"商誉","accountType":"asset"},
 {"schemeCode":"O2_TEST","accountCode":"6701","name":"资产减值损失","accountType":"expense"},
 {"schemeCode":"O2_TEST","accountCode":"4001","name":"实收资本","accountType":"equity"},
 {"schemeCode":"O2_TEST","accountCode":"4104","name":"未分配利润","accountType":"equity"},
 {"schemeCode":"O2_TEST","accountCode":"4400","name":"少数股东权益","accountType":"nci"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/elim-rules -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O2_TEST","ruleCode":"R_CAPITAL","elimType":"capital","drAccount":"","crAccount":"","enabled":1},
 {"schemeCode":"O2_TEST","ruleCode":"R_GOODWILL","elimType":"goodwill_impair","drAccount":"6701","crAccount":"","enabled":1}]}' >/dev/null
curl -s -m8 -X POST $B/consol/scope -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O2_TEST","periodCode":"2026-01","orgCode":"GRP","orgName":"集团","parentCode":"","consolMethod":"full","ownershipPct":1,"isLeaf":0,"levelNo":1},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","orgCode":"P","orgName":"母","parentCode":"GRP","consolMethod":"full","ownershipPct":1,"isLeaf":1,"levelNo":2},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","orgCode":"S","orgName":"子","parentCode":"GRP","consolMethod":"full","ownershipPct":1,"isLeaf":1,"levelNo":2,"investmentAmount":120}]}' >/dev/null
curl -s -m8 -X POST $B/consol/entity-balances -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O2_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"1001","amount":80},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"1511","amount":120},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4001","amount":-150},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4104","amount":-50},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"1001","amount":100},
 {"schemeCode":"O2_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"4001","amount":-100}]}' >/dev/null
# CGU:GRP 节点,账面 200 / 可收回 185 → 减值 15(封顶商誉 20)。
curl -s -m8 -X POST $B/consol/goodwill-cgu -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O2_TEST","periodCode":"2026-01","nodeCode":"GRP","carryingAmount":200,"recoverableAmount":185,"goodwillCarrying":20}]}' >/dev/null
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"O2_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/worksheet?scheme=O2_TEST&period=2026-01&node=GRP")
gw=$(echo "$r" | g "next((x['consolidated'] for x in d['data']['rows'] if x['account_code']=='1801'),'0')")
imp=$(echo "$r" | g "next((x['adjust'] for x in d['data']['rows'] if x['account_code']=='6701'),'0')")
bs=$(echo "$r" | g "round(sum(float(x['consolidated']) for x in d['data']['rows']),4)")
[ "$(near "$gw" 5)" = "1" ] && ok "O2 商誉减值后 consolidated=5(20−15)" || no "O2 商誉" "gw=$gw"
[ "$(near "$imp" 15)" = "1" ] && ok "O2 减值损失 adjust=+15" || no "O2 减值损失" "imp=$imp"
[ "$bs" = "0.0" ] && ok "O2 BS 恒等=0" || no "O2 BS" "bs=$bs"

echo "=== O3 自动 IC 调整建议(读 cg_ic_recon diff 行) ==="
curl -s -m8 -X POST $B/consol/schemes -H 'content-type: application/json' -d '{"schemeCode":"O3_TEST","name":"IC 调整建议测试","standard":"CAS","groupCurrency":"CNY","investmentAccount":"1511","goodwillAccount":"1801","nciAccount":"4400","minorityPlAccount":"4900","ctaAccount":"4106","capitalReserveAccount":"4002"}' >/dev/null
curl -s -m8 -X POST $B/consol/ic-declarations -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O3_TEST","periodCode":"2026-01","entityCode":"P","partnerCode":"S","icType":"debt","direction":"receivable","amount":100},
 {"schemeCode":"O3_TEST","periodCode":"2026-01","entityCode":"S","partnerCode":"P","icType":"debt","direction":"payable","amount":80}]}' >/dev/null
curl -s -m10 -X POST $B/consol/ic-reconcile -H 'content-type: application/json' -d '{"scheme":"O3_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/ic-adjustment-suggestions?scheme=O3_TEST&period=2026-01")
cnt=$(echo "$r" | g "d['data']['count']")
adje=$(echo "$r" | g "d['data']['suggestions'][0]['adjust_entity']")
adja=$(echo "$r" | g "d['data']['suggestions'][0]['adjust_amount']")
[ "$cnt" -ge 1 ] 2>/dev/null && ok "O3 建议条数≥1(count=$cnt)" || no "O3 建议条数" "cnt=$cnt"
[ "$adje" = "S" ] && ok "O3 建议调整主体=S(少报方)" || no "O3 调整主体" "adje=$adje"
[ "$(near "$adja" 20)" = "1" ] && ok "O3 建议调整额=20(100−80)" || no "O3 调整额" "adja=$adja"

echo "=== O4/O5 完整 CCF(33 行)/ CSE(EC01–EC20)直接法流水 ==="
# 重 seed 四表(含 O4/O5 扩展模板)。
curl -s -m10 -X POST $B/consol/seed-statements -H 'content-type: application/json' -d '{}' >/dev/null
# O5 CF 直接法流水(节点级,orgCode=G):经营 CF01=100/CF04=-40,投资 CF11=50,筹资 CF21=30,汇率 CF31=5,期初 CF32=200。
curl -s -m8 -X POST $B/consol/cash-flow -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","activity":"operating","itemCode":"CF01","amount":100},
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","activity":"operating","itemCode":"CF04","amount":-40},
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","activity":"investing","itemCode":"CF11","amount":50},
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","activity":"financing","itemCode":"CF21","amount":30},
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","activity":"fx","itemCode":"CF31","amount":5},
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","activity":"opening","itemCode":"CF32","amount":200}]}' >/dev/null
# O4 CSE 权益变动流水(节点级):上年年末 EC01=-1000(贷),综合收益 EC11=-200(贷)。
curl -s -m8 -X POST $B/consol/equity-change -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","equityItem":"total","changeType":"opening","columnCode":"EC01","amount":-1000},
 {"schemeCode":"O45_TEST","periodCode":"2026-01","nodeCode":"G","equityItem":"total","changeType":"oci","columnCode":"EC11","amount":-200}]}' >/dev/null
# O5 CCF compute:33 行,断言 errorCount=0 + 经营净额 B9=60 + 净增加 B26=145 + 期末 B28=345。
r=$(curl -s -m12 -X POST "$B/report-design/reports/CCF/compute" -H 'content-type: application/json' -d '{"orgCode":"G","periodCode":"2026-01","schemeCode":"O45_TEST"}')
echo "$r" | python3 -c '
import sys,json
d=json.load(sys.stdin)["data"]
c={x["cellRef"]:x["value"] for x in d["cells"]}
def chk(ref,exp,label):
    v=c.get(ref); ok=False
    try: ok=abs(float(v)-exp)<0.005
    except: pass
    print(("PASS" if ok else "FAIL")+f"|CCF {ref} {label}={v} exp={exp}")
ec=d["errorCount"]; nc=len(d["cells"])
print(("PASS" if ec==0 else "FAIL")+f"|O5 CCF 零计算错误 errors={ec}")
print(("PASS" if nc>=40 else "FAIL")+f"|O5 CCF 单元格数≥40(33 行×~2 列)={nc}")
chk("B9",60.0,"经营活动净额")
chk("B17",50.0,"投资活动净额")
chk("B24",30.0,"筹资活动净额")
chk("B26",145.0,"现金净增加额")
chk("B28",345.0,"期末现金余额")
' | while IFS='|' read st msg; do [ "$st" = "PASS" ] && ok "$msg" || no "$msg" ""; done
# O4 CSE compute:断言 errorCount=0 + 本年年初 B3=1000 + 本年年末 B16=1200。
r=$(curl -s -m12 -X POST "$B/report-design/reports/CSE/compute" -H 'content-type: application/json' -d '{"orgCode":"G","periodCode":"2026-01","schemeCode":"O45_TEST"}')
echo "$r" | python3 -c '
import sys,json
d=json.load(sys.stdin)["data"]
c={x["cellRef"]:x["value"] for x in d["cells"]}
def chk(ref,exp,label):
    v=c.get(ref); ok=False
    try: ok=abs(float(v)-exp)<0.005
    except: pass
    print(("PASS" if ok else "FAIL")+f"|CSE {ref} {label}={v} exp={exp}")
ec=d["errorCount"]
print(("PASS" if ec==0 else "FAIL")+f"|O4 CSE 零计算错误 errors={ec}")
chk("B3",1000.0,"本年年初余额")
chk("B16",1200.0,"本年年末余额")
' | while IFS='|' read st msg; do [ "$st" = "PASS" ] && ok "$msg" || no "$msg" ""; done

echo "=== O6 有效持股接入 NCI 精算(交叉持股) ==="
curl -s -m8 -X POST $B/consol/schemes -H 'content-type: application/json' -d '{"schemeCode":"O6_TEST","name":"有效持股 NCI 测试","standard":"CAS","groupCurrency":"CNY","investmentAccount":"1511","goodwillAccount":"1801","nciAccount":"4400","minorityPlAccount":"4900","ctaAccount":"4106","capitalReserveAccount":"4002"}' >/dev/null
curl -s -m8 -X POST $B/consol/group-accounts -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O6_TEST","accountCode":"1001","name":"货币资金","accountType":"asset"},
 {"schemeCode":"O6_TEST","accountCode":"6001","name":"营业收入","accountType":"income"},
 {"schemeCode":"O6_TEST","accountCode":"4001","name":"实收资本","accountType":"equity"},
 {"schemeCode":"O6_TEST","accountCode":"4400","name":"少数股东权益","accountType":"nci"},
 {"schemeCode":"O6_TEST","accountCode":"4900","name":"少数股东损益","accountType":"income"}]}' >/dev/null
curl -s -m8 -X POST $B/consol/elim-rules -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O6_TEST","ruleCode":"R_NCI","elimType":"nci","drAccount":"4900","crAccount":"4400","enabled":1}]}' >/dev/null
# 直接持股 GRP→S 0.8;交叉:P→S 0.1;有效持股 S=0.9 → NCI 应用 0.9(不是 0.8)。
curl -s -m8 -X POST $B/consol/scope -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O6_TEST","periodCode":"2026-01","orgCode":"GRP","orgName":"集团","parentCode":"","consolMethod":"full","ownershipPct":1,"isLeaf":0,"levelNo":1},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","orgCode":"P","orgName":"母","parentCode":"GRP","consolMethod":"full","ownershipPct":1,"isLeaf":1,"levelNo":2},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","orgCode":"S","orgName":"子","parentCode":"GRP","consolMethod":"full","ownershipPct":0.8,"isLeaf":1,"levelNo":2}]}' >/dev/null
curl -s -m8 -X POST $B/consol/entity-balances -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O6_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"1001","amount":100},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","entityCode":"P","accountCode":"4001","amount":-100},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"1001","amount":180},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"6001","amount":-100},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","entityCode":"S","accountCode":"4001","amount":-80}]}' >/dev/null
curl -s -m8 -X POST $B/consol/shareholding -H 'content-type: application/json' -d '{"items":[
 {"schemeCode":"O6_TEST","periodCode":"2026-01","holder":"GRP","held":"P","pct":1.0,"isParent":1},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","holder":"GRP","held":"S","pct":0.8,"isParent":1},
 {"schemeCode":"O6_TEST","periodCode":"2026-01","holder":"P","held":"S","pct":0.1}]}' >/dev/null
# 有效持股确认(L6 口径):S≈0.9。
r=$(curl -s -m8 "$B/consol/effective-ownership?scheme=O6_TEST&period=2026-01")
effs=$(echo "$r" | g "float(d['data']['effective']['S'])")
[ "$(near "$effs" 0.9)" = "1" ] && ok "O6 有效持股 S=0.9(0.8 直接+0.1 间接)" || no "O6 有效持股" "effs=$effs"
curl -s -m10 -X POST $B/consol/run -H 'content-type: application/json' -d '{"scheme":"O6_TEST","period":"2026-01"}' >/dev/null
r=$(curl -s -m8 "$B/consol/worksheet?scheme=O6_TEST&period=2026-01&node=GRP")
mpl=$(echo "$r" | g "next((x['elim'] for x in d['data']['rows'] if x['account_code']=='4900'),'0')")
bs=$(echo "$r" | g "round(sum(float(x['consolidated']) for x in d['data']['rows']),4)")
# 有效 0.9 → NCI 损益=(1−0.9)×100=10;若误用直接 0.8 → 20。
[ "$(near "$mpl" 10)" = "1" ] && ok "O6 NCI 损益用有效持股=10(非直接 20)" || no "O6 NCI 损益" "mpl=$mpl"
[ "$bs" = "0.0" ] && ok "O6 BS 恒等=0" || no "O6 BS" "bs=$bs"

echo ""
echo "=== $pass passed / $fail failed ==="
[ "$fail" = "0" ] && exit 0 || exit 1
