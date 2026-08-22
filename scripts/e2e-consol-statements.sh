#!/usr/bin/env bash
# C7 出表 E2E:合并四表 seed + 计算态 + 断言(BS/IS 真取数,CF/SOCE 模板壳)。
# 前置:report-server :8092 在跑,fico 已 seed CAS_LEGAL(2026-06)+ IS_TEST(2026-01)。
set -u
B=http://localhost:8092/api
pass=0; fail=0
ok(){ echo "✅ $1"; pass=$((pass+1)); }
no(){ echo "❌ $1 — $2"; fail=$((fail+1)); }

echo "=== 1. seed 合并四表 ==="
r=$(curl -s -m 10 -H 'content-type: application/json' -X POST $B/consol/seed-statements -d '{}')
n=$(echo "$r" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)["data"]["reports"]))' 2>/dev/null)
[ "$n" = "4" ] && ok "四表定义已 seed(CBS/CIS/CCF/CSE)" || no "seed 四表" "reports=$n"

echo "=== 2. 确保合并数据在位 ==="
curl -s -m 8 -H 'content-type: application/json' -X POST $B/consol/run -d '{"scheme":"CAS_LEGAL","period":"2026-06"}' >/dev/null
curl -s -m 8 -H 'content-type: application/json' -X POST $B/consol/run -d '{"scheme":"IS_TEST","period":"2026-01"}' >/dev/null
ok "CAS_LEGAL/IS_TEST 已合并"

echo "=== 3. 合并资产负债表 CBS(CSCEC/2026-06) ==="
r=$(curl -s -m 12 -H 'content-type: application/json' -X POST "$B/report-design/reports/CBS/compute" -d '{"orgCode":"CSCEC","periodCode":"2026-06","schemeCode":"CAS_LEGAL"}')
echo "$r" | python3 -c '
import sys,json
d=json.load(sys.stdin)["data"]
c={x["cellRef"]:x["value"] for x in d["cells"]}
def chk(ref,exp,label):
    v=c.get(ref); ok=False
    try: ok=abs(float(v)-exp)<0.005
    except: pass
    print(("PASS" if ok else "FAIL")+f"|CBS {ref} {label}={v} exp={exp}")
chk("B6",360.0,"资产合计")
chk("B15",360.0,"权益合计")
chk("B16",360.0,"负债和权益合计")
a1=c.get("A1")
lbl = a1=="一、资产"
print(("PASS" if lbl else "FAIL")+"|CBS A1 标签渲染="+str(a1))
print(("PASS" if d["errorCount"]==0 else "FAIL")+"|CBS 零计算错误 errors="+str(d["errorCount"]))
' | while IFS='|' read st msg; do [ "$st" = "PASS" ] && ok "$msg" || no "$msg" ""; done

echo "=== 4. 合并利润表 CIS(IS_TEST G/2026-01) ==="
r=$(curl -s -m 12 -H 'content-type: application/json' -X POST "$B/report-design/reports/CIS/compute" -d '{"orgCode":"G","periodCode":"2026-01","schemeCode":"IS_TEST"}')
echo "$r" | python3 -c '
import sys,json
d=json.load(sys.stdin)["data"]
c={x["cellRef"]:x["value"] for x in d["cells"]}
def chk(ref,exp,label):
    v=c.get(ref); ok=False
    try: ok=abs(float(v)-exp)<0.005
    except: pass
    print(("PASS" if ok else "FAIL")+f"|CIS {ref} {label}={v} exp={exp}")
chk("B1",350.0,"营业收入")
chk("B4",130.0,"利润总额")
chk("B6",10.0,"少数股东损益")
chk("B7",120.0,"归母净利润")
' | while IFS='|' read st msg; do [ "$st" = "PASS" ] && ok "$msg" || no "$msg" ""; done

echo "=== 5. CF/SOCE 模板壳可计算(结构在,无数据错误) ==="
for rep in CCF CSE; do
  r=$(curl -s -m 12 -H 'content-type: application/json' -X POST "$B/report-design/reports/$rep/compute" -d '{"orgCode":"CSCEC","periodCode":"2026-06","schemeCode":"CAS_LEGAL"}')
  ec=$(echo "$r" | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["errorCount"])' 2>/dev/null)
  [ "$ec" = "0" ] && ok "$rep 模板壳计算无错(errors=0)" || no "$rep 模板壳" "errors=$ec"
done

echo ""
echo "=== $pass passed / $fail failed ==="
[ "$fail" = "0" ] && exit 0 || exit 1
