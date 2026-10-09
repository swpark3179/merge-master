<#
  와이어프레임(1a)의 '2026_4Q_예산신청_취합.xlsx'를 흉내 낸 테스트용 통합문서를 만든다.
  - 시트: 예산신청서 / 작성가이드 / 코드표
  - 예산신청서: 다단 머리글(1~2행) + 데이터 1,000행(3~1002행), A~I열
    병합(본부·부서·담당자), 수식(G열 증감률), 목록 유효성(I열 → 코드표!A2:A6), 서식, 틀 고정(E3)
  실행: pnpm fixture   (Excel 설치 필요, 백그라운드 인스턴스로 생성)
#>
param(
  [string]$OutPath = (Join-Path $PSScriptRoot '..\fixtures\generated\budget.xlsx')
)
$ErrorActionPreference = 'Stop'

$OutPath = [System.IO.Path]::GetFullPath($OutPath)
New-Item -ItemType Directory -Force -Path (Split-Path $OutPath) | Out-Null
if (Test-Path $OutPath) { Remove-Item $OutPath -Force }

# 재현 가능한 의사 난수 (LCG)
$script:seed = 20261008
# PowerShell COM 바인더가 2차원 배열 대입에서 InvalidCastException을 내는 경우가 있어 리플렉션으로 설정한다.
function Set-Value2($range, [object[,]]$values) {
  [void]$range.GetType().InvokeMember('Value2', [Reflection.BindingFlags]::SetProperty, $null, $range, @(,$values))
}

function Next-Rand([int]$max) {
  $script:seed = ($script:seed * 1103515245 + 12345) % 2147483648
  return [int]($script:seed % $max)
}

$divisions = @(
  @{ Name = '경영지원'; Depts = @('인사팀', '재무팀', '총무팀', '법무팀', '구매팀') },
  @{ Name = '영업';     Depts = @('국내영업팀', '국내영업 2팀', '해외영업팀', '영업기획팀', '고객지원팀') },
  @{ Name = '생산';     Depts = @('생산1팀', '생산2팀', '품질팀', '설비팀', '물류팀') },
  @{ Name = '연구';     Depts = @('선행연구팀', 'SW개발팀', 'HW개발팀', '시험인증팀', '디자인팀') }
)
$owners = @('이서연', '최도윤', '김민지', '정하늘', '한지우', '오세린')
$items  = @('교육훈련비', '회의비', '소모품비', '외주용역비', '출장비')
$basis  = @('리더십 교육 2회', '간담회 4회', '판촉물 소모분', '회계감사 계약', '거점 방문 월 4회', '전년 동일 수준 유지')
$timing = @('1분기', '2분기', '3분기', '4분기', '연중')

$dataRows = 1000
$firstData = 3
$lastRow = $firstData + $dataRows - 1   # 1002
$cols = 9                                # A..I

$xl = New-Object -ComObject Excel.Application
try {
  $xl.Visible = $false
  $xl.DisplayAlerts = $false
  $xl.ScreenUpdating = $false
  $wb = $xl.Workbooks.Add()
  while ($wb.Worksheets.Count -lt 3) { [void]$wb.Worksheets.Add([Type]::Missing, $wb.Worksheets.Item($wb.Worksheets.Count)) }
  $ws    = $wb.Worksheets.Item(1); $ws.Name = '예산신청서'
  $guide = $wb.Worksheets.Item(2); $guide.Name = '작성가이드'
  $codes = $wb.Worksheets.Item(3); $codes.Name = '코드표'

  # --- 코드표 ---
  $codes.Range('A1').Value2 = '집행 시기'
  for ($i = 0; $i -lt $timing.Count; $i++) { $codes.Cells.Item($i + 2, 1).Value2 = $timing[$i] }
  $codes.Range('A1').Font.Bold = $true

  # --- 작성가이드 ---
  $guide.Range('A1').Value2 = '작성 가이드'
  $guide.Range('A2').Value2 = '1. 신청 금액은 천원 단위 숫자로 입력합니다.'
  $guide.Range('A3').Value2 = '2. 산출 근거는 "단가 × 수량" 형식으로 적습니다.'
  $guide.Range('A4').Value2 = '3. 집행 시기는 목록에서 고릅니다.'
  $guide.Range('A1').Font.Bold = $true

  # --- 예산신청서: 머리글 ---
  $ws.Range('A1').Value2 = '기본 정보'
  $ws.Range('E1').Value2 = '금액 (천원)'
  $ws.Range('H1').Value2 = '세부 내용'
  [void]$ws.Range('A1:D1').Merge()
  [void]$ws.Range('E1:G1').Merge()
  [void]$ws.Range('H1:I1').Merge()
  $heads = @('본부', '부서', '담당자', '비목', '전년 실적', '신청 금액', '증감률', '산출 근거', '집행 시기')
  $h = [object[,]]::new(1, $cols)
  for ($c = 0; $c -lt $cols; $c++) { $h[0, $c] = [string]$heads[$c] }
  Set-Value2 $ws.Range('A2:I2') $h

  # --- 데이터 (값 일괄 쓰기) ---
  $vals = [object[,]]::new($dataRows, $cols)
  $blocks = New-Object System.Collections.ArrayList   # @{ Start; End; Kind }
  $r = 0; $deptIndex = 0
  while ($r -lt $dataRows) {
    foreach ($div in $divisions) {
      $divStart = $r
      foreach ($dept in $div.Depts) {
        if ($r -ge $dataRows) { break }
        $deptStart = $r
        $owner = $owners[$deptIndex % $owners.Count]
        foreach ($item in $items) {
          if ($r -ge $dataRows) { break }
          if ($r -eq $deptStart) {
            $vals[$r, 1] = [string]$dept
            $vals[$r, 2] = [string]$owner
          }
          if ($r -eq $divStart) { $vals[$r, 0] = [string]$div.Name }
          $vals[$r, 3] = [string]$item
          $vals[$r, 4] = [double]((Next-Rand 90 + 1) * 100 + 1000)
          if ((Next-Rand 3) -eq 0) {
            $vals[$r, 5] = [double]([double]$vals[$r, 4] + (Next-Rand 20) * 100)
            $vals[$r, 7] = [string]$basis[(Next-Rand $basis.Count)]
            $vals[$r, 8] = [string]$timing[(Next-Rand $timing.Count)]
          }
          $r++
        }
        [void]$blocks.Add(@{ Start = $deptStart; End = $r - 1; Col = 'B' })
        [void]$blocks.Add(@{ Start = $deptStart; End = $r - 1; Col = 'C' })
        $deptIndex++
      }
      if ($r -gt $divStart) { [void]$blocks.Add(@{ Start = $divStart; End = $r - 1; Col = 'A' }) }
      if ($r -ge $dataRows) { break }
    }
  }
  Set-Value2 $ws.Range("A${firstData}:I${lastRow}") $vals

  # 수식: G열 증감률
  $ws.Range("G${firstData}:G${lastRow}").Formula = '=IF(OR(E3="",F3=""),"",F3/E3-1)'

  # 병합 (본부·부서·담당자 블록)
  foreach ($b in $blocks) {
    if ($b.End -gt $b.Start) {
      $a = "$($b.Col)$($b.Start + $firstData):$($b.Col)$($b.End + $firstData)"
      [void]$ws.Range($a).Merge()
    }
  }

  # --- 서식 ---
  $all = $ws.Range("A1:I${lastRow}")
  $all.Font.Name = '맑은 고딕'
  $all.Font.Size = 10
  $all.Borders.LineStyle = 1        # xlContinuous
  $all.Borders.Weight = 2           # xlThin
  $all.Borders.Color = 0xBFBFBF

  $head = $ws.Range('A1:I2')
  $head.Font.Bold = $true
  $head.HorizontalAlignment = -4108 # xlCenter
  $head.VerticalAlignment = -4108
  $head.Interior.Color = 0xEBE7E3   # BGR
  $ws.Range('A1:I1').Interior.Color = 0xD9D2CB
  $ws.Range('F2').Interior.Color = 0xF8E8DB

  $ws.Range("A${firstData}:C${lastRow}").HorizontalAlignment = -4108
  $ws.Range("A${firstData}:C${lastRow}").VerticalAlignment = -4108
  $ws.Range("A${firstData}:B${lastRow}").Interior.Color = 0xF3F6F7
  $ws.Range("E${firstData}:F${lastRow}").NumberFormat = '#,##0'
  $ws.Range("G${firstData}:G${lastRow}").NumberFormat = '+0.0%;-0.0%;0.0%'
  $ws.Range("G${firstData}:G${lastRow}").Font.Color = 0x7F7F7F
  $ws.Range("I${firstData}:I${lastRow}").HorizontalAlignment = -4108

  # 목록 유효성: I열 → 코드표!A2:A6
  $v = $ws.Range("I${firstData}:I${lastRow}").Validation
  $v.Delete()
  [void]$v.Add(3, 1, 1, '=코드표!$A$2:$A$6')   # xlValidateList, xlValidAlertStop, xlBetween

  # 열 너비 · 행 높이
  $widths = @(9, 11, 8, 11, 10, 10, 8, 28, 9)
  for ($c = 0; $c -lt $cols; $c++) { $ws.Columns.Item($c + 1).ColumnWidth = $widths[$c] }
  $ws.Rows.Item(1).RowHeight = 20
  $ws.Rows.Item(2).RowHeight = 24

  # 틀 고정: 머리글 2행 + A~D열
  $ws.Activate()
  $xl.ScreenUpdating = $true
  $win = $xl.ActiveWindow
  $win.FreezePanes = $false
  $win.SplitColumn = 4
  $win.SplitRow = 2
  $win.FreezePanes = $true
  $ws.Range('F3').Select()

  $wb.SaveAs($OutPath, 51)          # xlOpenXMLWorkbook
  $wb.Close($false)
  Write-Host "생성 완료: $OutPath  (데이터 $dataRows 행, 병합 블록 $($blocks.Count)개)"
}
finally {
  $xl.Quit()
  [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($xl)
  [GC]::Collect(); [GC]::WaitForPendingFinalizers()
}
