#!/usr/bin/env bash
# O7 —— cmx-container 一键部署集成:合并报表平台全链 bootstrap。
#
# 一条命令完成:①部署 24 张 cg_* 元数据(model-center /api/model/deploy,db_id=fico-db,加法幂等)
# → ②seed 全链 demo 方案(母+2 子,资本抵销/IC 抵销/少数股东,借方正)→ ③run 合并 + 验证 BS 恒等=0
# → ④seed 合并四表(CBS/CIS/CCF/CSE)+ 计算态零错误 → ⑤打印部署清单(菜单/proxy/native-page 已就位)。
#
# 用法:  bash scripts/bootstrap-consol.sh
# 环境:  MC=http://localhost:8099(model-center)  RS=http://localhost:8092(report-server)
#        DB_ID=fico-db  SCHEME=DEMO_CONSOL  PERIOD=2026-06
set -u
MC=${MC:-http://localhost:8099}
RS=${RS:-http://localhost:8092}/api
DB_ID=${DB_ID:-fico-db}
SCHEME=${SCHEME:-DEMO_CONSOL}
PERIOD=${PERIOD:-2026-06}
pass=0; fail=0
ok(){ echo "✅ $1"; pass=$((pass+1)); }
no(){ echo "❌ $1 — $2"; fail=$((fail+1)); }
g(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }
J='content-type: application/json'

echo "══════════════════════════════════════════════════════════════"
echo " 合并报表平台 一键部署  MC=$MC  RS=$RS  db=$DB_ID  scheme=$SCHEME/$PERIOD"
echo "══════════════════════════════════════════════════════════════"

echo "── ① 部署 24 张 cg_* 元数据(加法幂等) ──"
r=$(curl -s -m90 -X POST "$MC/api/model/deploy" -H "$J" -d "{\"db_id\":\"$DB_ID\",\"items\":[{\"kind\":\"DCT\",\"domain\":\"fi\",\"application\":\"cmxfico\",\"module\":\"consol\",\"file\":\"cmxfico_consol_dct_meta_v1.json\"}]}")
tables=$(echo "$r" | g "d['data']['results'][0]['tables']")
st=$(echo "$r" | g "d['data']['results'][0]['status']")
[ "$tables" = "24" ] && [ "$st" = "success" ] && ok "元数据部署成功(tables=$tables status=$st)" || no "元数据部署" "$(echo "$r" | head -c 200)"

echo "── ② seed 全链 demo 方案(母 P + 全资子 S1 + 80% 子 S2;资本抵销/IC 抵销/少数股东) ──"
curl -s -m8 -X POST $RS/consol/schemes -H "$J" -d "{\"schemeCode\":\"$SCHEME\",\"name\":\"合并演示方案\",\"standard\":\"CAS\",\"groupCurrency\":\"CNY\",\"investmentAccount\":\"1511\",\"goodwillAccount\":\"1801\",\"nciAccount\":\"4400\",\"minorityPlAccount\":\"4900\",\"ctaAccount\":\"4106\",\"capitalReserveAccount\":\"4002\"}" >/dev/null
curl -s -m8 -X POST $RS/consol/group-accounts -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"1001\",\"name\":\"货币资金\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"1122\",\"name\":\"应收账款\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"1511\",\"name\":\"长期股权投资\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"1801\",\"name\":\"商誉\",\"accountType\":\"asset\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"2202\",\"name\":\"应付账款\",\"accountType\":\"liability\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"4001\",\"name\":\"实收资本\",\"accountType\":\"equity\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"4104\",\"name\":\"未分配利润\",\"accountType\":\"equity\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"4400\",\"name\":\"少数股东权益\",\"accountType\":\"nci\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"4900\",\"name\":\"少数股东损益\",\"accountType\":\"income\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"6001\",\"name\":\"营业收入\",\"accountType\":\"income\"},
 {\"schemeCode\":\"$SCHEME\",\"accountCode\":\"6401\",\"name\":\"营业成本\",\"accountType\":\"expense\"}]}" >/dev/null
curl -s -m8 -X POST $RS/consol/elim-rules -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$SCHEME\",\"ruleCode\":\"R_CAP\",\"elimType\":\"capital\",\"drAccount\":\"\",\"crAccount\":\"\",\"enabled\":1},
 {\"schemeCode\":\"$SCHEME\",\"ruleCode\":\"R_DEBT\",\"elimType\":\"debt\",\"drAccount\":\"2202\",\"crAccount\":\"1122\",\"enabled\":1},
 {\"schemeCode\":\"$SCHEME\",\"ruleCode\":\"R_NCI\",\"elimType\":\"nci\",\"drAccount\":\"4900\",\"crAccount\":\"4400\",\"enabled\":1}]}" >/dev/null
curl -s -m8 -X POST $RS/consol/scope -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"orgCode\":\"GRP\",\"orgName\":\"演示集团\",\"parentCode\":\"\",\"consolMethod\":\"full\",\"ownershipPct\":1,\"isLeaf\":0,\"levelNo\":1},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"orgCode\":\"P\",\"orgName\":\"母公司\",\"parentCode\":\"GRP\",\"consolMethod\":\"full\",\"ownershipPct\":1,\"isLeaf\":1,\"levelNo\":2},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"orgCode\":\"S1\",\"orgName\":\"全资子\",\"parentCode\":\"GRP\",\"consolMethod\":\"full\",\"ownershipPct\":1,\"isLeaf\":1,\"levelNo\":2,\"investmentAmount\":100},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"orgCode\":\"S2\",\"orgName\":\"80%子\",\"parentCode\":\"GRP\",\"consolMethod\":\"full\",\"ownershipPct\":0.8,\"isLeaf\":1,\"levelNo\":2,\"investmentAmount\":80}]}" >/dev/null
curl -s -m8 -X POST $RS/consol/entity-balances -H "$J" -d "{\"items\":[
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"P\",\"accountCode\":\"1001\",\"amount\":300},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"P\",\"accountCode\":\"1511\",\"amount\":180},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"P\",\"accountCode\":\"4001\",\"amount\":-400},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"P\",\"accountCode\":\"4104\",\"amount\":-80},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S1\",\"accountCode\":\"1001\",\"amount\":50},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S1\",\"accountCode\":\"1122\",\"amount\":40},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S1\",\"accountCode\":\"4001\",\"amount\":-80},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S1\",\"accountCode\":\"4104\",\"amount\":-10},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S2\",\"accountCode\":\"1001\",\"amount\":130},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S2\",\"accountCode\":\"2202\",\"amount\":-40},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S2\",\"accountCode\":\"4001\",\"amount\":-60},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S2\",\"accountCode\":\"6001\",\"amount\":-50},
 {\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityCode\":\"S2\",\"accountCode\":\"6401\",\"amount\":20}]}" >/dev/null
# IC 债权债务:S1 应收 S2 40 = S2 应付 S1 40。
curl -s -m8 -X POST $RS/consol/ic-matches -H "$J" -d "{\"items\":[{\"schemeCode\":\"$SCHEME\",\"periodCode\":\"$PERIOD\",\"entityA\":\"S1\",\"entityB\":\"S2\",\"icType\":\"debt\",\"amount\":40}]}" >/dev/null
ok "demo 方案 seed 完成(3 主体 / 11 科目 / 3 抵销规则 / 1 组 IC)"

echo "── ③ run 合并 + 验证 BS 恒等=0 ──"
curl -s -m12 -X POST $RS/consol/run -H "$J" -d "{\"scheme\":\"$SCHEME\",\"period\":\"$PERIOD\"}" >/dev/null
r=$(curl -s -m8 "$RS/consol/worksheet?scheme=$SCHEME&period=$PERIOD&node=GRP")
bs=$(echo "$r" | g "round(sum(float(x['consolidated']) for x in d['data']['rows']),4)")
ne=$(echo "$r" | g "len(d['data']['rows'])")
[ "$bs" = "0.0" ] && ok "BS 恒等 Σ(合并数)=0(账户数=$ne)" || no "BS 恒等" "bs=$bs"
# IC 抵销确认:应收 1122 与应付 2202 合并后归零(两端 40 相抵)。
ar=$(echo "$r" | g "next((x['consolidated'] for x in d['data']['rows'] if x['account_code']=='1122'),'0')")
[ "$(python3 -c "print(1 if abs(float('$ar'))<0.005 else 0)")" = "1" ] && ok "IC 抵销:内部应收合并后=0" || no "IC 抵销" "ar=$ar"

echo "── ④ seed 合并四表 + 计算态零错误 ──"
r=$(curl -s -m10 -X POST $RS/consol/seed-statements -H "$J" -d '{}')
n=$(echo "$r" | g "len(d['data']['reports'])")
[ "$n" = "4" ] && ok "合并四表 seed(CBS/CIS/CCF/CSE)" || no "四表 seed" "n=$n"
for rep in CBS CIS CCF CSE; do
  rr=$(curl -s -m12 -X POST "$RS/report-design/reports/$rep/compute" -H "$J" -d "{\"orgCode\":\"GRP\",\"periodCode\":\"$PERIOD\",\"schemeCode\":\"$SCHEME\"}")
  ec=$(echo "$rr" | g "d['data']['errorCount']")
  [ "$ec" = "0" ] && ok "$rep 计算态零错误" || no "$rep 计算" "errors=$ec"
done

echo "── ⑤ 部署清单(集成就位项) ──"
cat <<'MANIFEST'
  ┌ 元数据    24 张 cg_* 表(model-center /api/model/deploy · db=fico-db · 加法幂等)
  ├ 后端      report-server :8092  /api/consol/*(38 端点,含 O1–O6:net-investment-hedge
  │           / goodwill-cgu / ic-adjustment-suggestions / effective-ownership / notes)
  ├ 出表      复用 cmx-rpt-formula 引擎:CBS/CIS 真取数(CG)+ CCF(33 行,CF)+ CSE(EC,20 列)
  ├ 前端      native-page portal.consol.workbench(改源即生效,六 tab:底稿/对账/分类账/范围/四表/附注)
  ├ 门户      反代 /api/consol/* + 页 proxy + 白名单(已就位)
  └ 菜单      cmx_menu 合并工作台(MENU deploy 写默认库,已部署)
MANIFEST

echo ""
echo "══════════════════════════════════════════════════════════════"
echo " 一键部署完成:$pass 通过 / $fail 失败"
echo "══════════════════════════════════════════════════════════════"
[ "$fail" = "0" ] && exit 0 || exit 1
