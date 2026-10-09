<#
  불러오기(1a) 감지 규칙을 시험하는 변형 통합문서 fixtures/generated/variants.xlsx 를 만든다.
  - '제목있음' 시트
      1행: 제목(A1:G1 병합) / 2행: 빈 행 / 3행: 작성 기준일 안내(한 칸)
      4~5행: 머리글. 부서·담당자·비고·작성자 확인은 세로 병합(4~5행), '교육 과정'은 C4:E4 가로 병합,
             5행(과정명·인원·시간)은 굵지 않음
      6~25행: 데이터 20행. 12행은 숨김, F열(비고)은 숨김
      26~30행: 테두리만 있는 빈 행(사용 범위에는 들어가지만 표가 아님)
      틀 고정: C6 (5행 · A~B열)
  - '다른시트' 시트: 저장할 때 활성 시트. 틀 고정 B2.
  실행: pnpm fixture
#>
param(
  [string]$OutPath = (Join-Path $PSScriptRoot '..\fixtures\generated\variants.xlsx')
)
$ErrorActionPreference = 'Stop'
$OutPath = [System.IO.Path]::GetFullPath($OutPath)
New-Item -ItemType Directory -Force -Path (Split-Path $OutPath) | Out-Null
if (Test-Path $OutPath) { Remove-Item $OutPath -Force }

function Set-Value2($range, [object[,]]$values) {
  [void]$range.GetType().InvokeMember('Value2', [Reflection.BindingFlags]::SetProperty, $null, $range, @(,$values))
}

function Freeze($xl, $sheet, [string]$cell) {
  $sheet.Activate()
  $win = $xl.ActiveWindow
  $win.FreezePanes = $false
  $sheet.Range($cell).Select()
  $win.FreezePanes = $true
}

$xl = New-Object -ComObject Excel.Application
try {
  $xl.Visible = $false
  $xl.DisplayAlerts = $false
  $wb = $xl.Workbooks.Add()
  while ($wb.Worksheets.Count -lt 2) { [void]$wb.Worksheets.Add([Type]::Missing, $wb.Worksheets.Item($wb.Worksheets.Count)) }
  $ws = $wb.Worksheets.Item(1); $ws.Name = '제목있음'
  $other = $wb.Worksheets.Item(2); $other.Name = '다른시트'

  $ws.Range('A1').Value2 = '2026년 하반기 교육 수요 조사'
  [void]$ws.Range('A1:G1').Merge()
  $ws.Range('A1').Font.Bold = $true
  $ws.Range('A1').Font.Size = 14
  $ws.Range('A3').Value2 = '작성 기준일: 2026-10-01'

  foreach ($m in @(@('A4', '부서', 'A4:A5'), @('B4', '담당자', 'B4:B5'), @('F4', '비고', 'F4:F5'), @('G4', '작성자 확인', 'G4:G5'), @('C4', '교육 과정', 'C4:E4'))) {
    $ws.Range($m[0]).Value2 = [string]$m[1]
    [void]$ws.Range($m[2]).Merge()
  }
  $ws.Range('C5').Value2 = '과정명'
  $ws.Range('D5').Value2 = '인원'
  $ws.Range('E5').Value2 = '시간'
  $ws.Range('A4:G4').Font.Bold = $true
  $ws.Range('A4:G5').HorizontalAlignment = -4108

  $depts  = @('인사팀', '재무팀', '총무팀', '구매팀')
  $people = @('이서연', '최도윤', '김민지', '정하늘')
  $courses = @('리더십 과정', '엑셀 실무', '보고서 작성', '데이터 분석', '안전 교육')
  $vals = [object[,]]::new(20, 7)
  for ($i = 0; $i -lt 20; $i++) {
    $vals[$i, 0] = [string]$depts[[int][math]::Floor($i / 5)]
    $vals[$i, 1] = [string]$people[[int][math]::Floor($i / 5)]
    $vals[$i, 2] = [string]$courses[$i % 5]
    $vals[$i, 3] = [double](($i % 7) + 3)
    $vals[$i, 4] = [double](8 * (($i % 3) + 1))
    if ($i % 4 -eq 0) { $vals[$i, 5] = [string]'사내 강사' }
  }
  Set-Value2 $ws.Range('A6:G25') $vals

  $ws.Range('A4:G30').Borders.LineStyle = 1
  $ws.Rows.Item(12).Hidden = $true
  $ws.Columns.Item(6).Hidden = $true

  $other.Range('A1').Value2 = '메모'
  $other.Range('A2').Value2 = '이 시트는 표가 아닙니다'

  Freeze $xl $ws 'C6'
  Freeze $xl $other 'B2'   # 마지막으로 활성화한 '다른시트'가 저장 시 활성 시트

  $wb.SaveAs($OutPath, 51)
  $wb.Close($false)
  Write-Host "생성 완료: $OutPath"
}
finally {
  $xl.Quit()
  [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($xl)
  [GC]::Collect(); [GC]::WaitForPendingFinalizers()
}
