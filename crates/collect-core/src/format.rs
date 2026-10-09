//! 1b 서식 도구 줄: 호스트가 불러온 서식을 조정한다(참여자 화면에 그대로 보인다).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};
use crate::sheet::{BorderEdge, CellStyle, HAlign, SheetModel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum FillPatch {
    None,
    Color { rgb: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BorderPreset {
    /// 모든 테두리(가는 실선).
    All,
    /// 바깥쪽 테두리만.
    Outline,
    None,
}

/// 바꿀 속성만 채운다.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct StylePatch {
    #[ts(optional)]
    pub font_name: Option<String>,
    #[ts(optional)]
    pub font_size: Option<f32>,
    #[ts(optional)]
    pub bold: Option<bool>,
    #[ts(optional)]
    pub italic: Option<bool>,
    #[ts(optional)]
    pub underline: Option<bool>,
    #[ts(optional)]
    pub color: Option<u32>,
    #[ts(optional)]
    pub fill: Option<FillPatch>,
    #[ts(optional)]
    pub h_align: Option<HAlign>,
    #[ts(optional)]
    pub borders: Option<BorderPreset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum FormatEdit {
    Style { range: CellRange, patch: StylePatch },
    Merge { range: CellRange },
    Unmerge { range: CellRange },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("범위가 표 밖입니다: {0}")]
    OutOfBounds(CellRange),
    #[error("두 칸 이상을 골라 주세요")]
    SingleCell,
    #[error("다른 병합 영역과 일부만 겹칩니다: {0}")]
    PartialMerge(CellRange),
    #[error("병합하면 왼쪽 위 값만 남습니다. 다른 칸의 값을 먼저 지워 주세요 ({0})")]
    WouldLoseData(CellRef),
}

const THIN: BorderEdge = BorderEdge { style: 1, weight: 2, color: 0x000000 };

pub fn apply_format(model: &mut SheetModel, edit: FormatEdit) -> Result<(), FormatError> {
    let bounds = model.bounds();
    let clip = |r: CellRange| r.intersect(&bounds).ok_or(FormatError::OutOfBounds(r));
    match edit {
        FormatEdit::Style { range, patch } => {
            let range = clip(range)?;
            for at in range.cells() {
                let s = model.cell(at).map_or(0, |c| c.s) as usize;
                let mut style = model.styles.get(s).cloned().unwrap_or_default();
                patch_style(&mut style, &patch, at, range);
                let id = intern(&mut model.styles, style);
                if let Some(cell) = model.cell_mut(at) {
                    cell.s = id;
                }
            }
            if patch.borders == Some(BorderPreset::None) {
                clear_facing_edges(model, range);
            }
        }
        FormatEdit::Merge { range } => {
            let range = clip(range)?;
            if range.is_single() {
                return Err(FormatError::SingleCell);
            }
            if let Some(m) = model.merges.iter().find(|m| m.intersect(&range).is_some() && !range.contains_range(m)) {
                return Err(FormatError::PartialMerge(*m));
            }
            if let Some(at) = range.cells().skip(1).find(|&at| model.cell(at).is_some_and(|c| !c.v.is_empty())) {
                return Err(FormatError::WouldLoseData(at));
            }
            model.merges.retain(|m| !range.contains_range(m));
            model.merges.push(range);
            model.merges.sort_by_key(|m| (m.row, m.col));
        }
        FormatEdit::Unmerge { range } => {
            let range = clip(range)?;
            model.merges.retain(|m| m.intersect(&range).is_none());
        }
    }
    Ok(())
}

fn patch_style(s: &mut CellStyle, p: &StylePatch, at: CellRef, range: CellRange) {
    if let Some(v) = &p.font_name {
        s.font_name = v.clone();
    }
    if let Some(v) = p.font_size {
        s.font_size = v.clamp(6.0, 72.0);
    }
    if let Some(v) = p.bold {
        s.bold = v;
    }
    if let Some(v) = p.italic {
        s.italic = v;
    }
    if let Some(v) = p.underline {
        s.underline = v;
    }
    if let Some(v) = p.color {
        s.color = v & 0xFF_FFFF;
    }
    match p.fill {
        Some(FillPatch::None) => s.fill = None,
        Some(FillPatch::Color { rgb }) => s.fill = Some(rgb & 0xFF_FFFF),
        None => {}
    }
    if let Some(v) = p.h_align {
        s.h_align = v;
    }
    match p.borders {
        Some(BorderPreset::All) => {
            s.borders.top = Some(THIN);
            s.borders.right = Some(THIN);
            s.borders.bottom = Some(THIN);
            s.borders.left = Some(THIN);
        }
        Some(BorderPreset::Outline) => {
            if at.row == range.row {
                s.borders.top = Some(THIN);
            }
            if at.row == range.last_row() {
                s.borders.bottom = Some(THIN);
            }
            if at.col == range.col {
                s.borders.left = Some(THIN);
            }
            if at.col == range.last_col() {
                s.borders.right = Some(THIN);
            }
        }
        Some(BorderPreset::None) => s.borders = Default::default(),
        None => {}
    }
}

/// 테두리 없애기: 범위 바로 바깥 이웃 셀이 범위 쪽으로 그린 선(같은 선)도 지운다.
fn clear_facing_edges(model: &mut SheetModel, range: CellRange) {
    let mut edits: Vec<(CellRef, fn(&mut CellStyle))> = Vec::new();
    for c in range.col..=range.last_col() {
        if range.row > 0 {
            edits.push((CellRef::new(range.row - 1, c), |s| s.borders.bottom = None));
        }
        edits.push((CellRef::new(range.last_row() + 1, c), |s| s.borders.top = None));
    }
    for r in range.row..=range.last_row() {
        if range.col > 0 {
            edits.push((CellRef::new(r, range.col - 1), |s| s.borders.right = None));
        }
        edits.push((CellRef::new(r, range.last_col() + 1), |s| s.borders.left = None));
    }
    for (at, f) in edits {
        let Some(s) = model.cell(at).map(|c| c.s as usize) else { continue };
        let mut style = model.styles.get(s).cloned().unwrap_or_default();
        f(&mut style);
        let id = intern(&mut model.styles, style);
        if let Some(cell) = model.cell_mut(at) {
            cell.s = id;
        }
    }
}

/// 같은 서식은 하나만 둔다.
fn intern(styles: &mut Vec<CellStyle>, style: CellStyle) -> u32 {
    match styles.iter().position(|s| *s == style) {
        Some(i) => i as u32,
        None => {
            styles.push(style);
            (styles.len() - 1) as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::budget;

    fn r(a: &str) -> CellRange {
        CellRange::parse(a).unwrap()
    }

    fn style_at<'a>(m: &'a SheetModel, a: &str) -> &'a CellStyle {
        &m.styles[m.cell(CellRef::parse(a).unwrap()).unwrap().s as usize]
    }

    #[test]
    fn style_patch_interns_styles() {
        let mut m = budget();
        let before = m.styles.len();
        let patch = StylePatch {
            bold: Some(true),
            fill: Some(FillPatch::Color { rgb: 0xFFF3B0 }),
            h_align: Some(HAlign::Center),
            ..Default::default()
        };
        apply_format(&mut m, FormatEdit::Style { range: r("F3:F8"), patch }).unwrap();
        let s = style_at(&m, "F5");
        assert!(s.bold && s.fill == Some(0xFFF3B0) && s.h_align == HAlign::Center);
        assert_eq!(s.number_format, "#,##0"); // 바꾸지 않은 속성은 그대로
        assert_eq!(m.styles.len(), before + 1); // F열 6칸이 같은 서식 하나를 공유
        apply_format(
            &mut m,
            FormatEdit::Style {
                range: r("F3"),
                patch: StylePatch { fill: Some(FillPatch::None), ..Default::default() },
            },
        )
        .unwrap();
        assert_eq!(style_at(&m, "F3").fill, None);
    }

    #[test]
    fn border_presets() {
        let mut m = budget();
        let outline = StylePatch { borders: Some(BorderPreset::Outline), ..Default::default() };
        apply_format(&mut m, FormatEdit::Style { range: r("H3:I4"), patch: outline }).unwrap();
        assert!(style_at(&m, "H3").borders.top.is_some() && style_at(&m, "H3").borders.left.is_some());
        assert!(style_at(&m, "H3").borders.bottom.is_none() && style_at(&m, "H3").borders.right.is_none());
        assert!(style_at(&m, "I4").borders.bottom.is_some() && style_at(&m, "I4").borders.right.is_some());

        let all = StylePatch { borders: Some(BorderPreset::All), ..Default::default() };
        apply_format(&mut m, FormatEdit::Style { range: r("H5:H6"), patch: all }).unwrap();
        let none = StylePatch { borders: Some(BorderPreset::None), ..Default::default() };
        apply_format(&mut m, FormatEdit::Style { range: r("H4"), patch: none }).unwrap();
        // H4 아래 선은 H5의 위 선과 같은 선이라 함께 지운다.
        assert!(style_at(&m, "H5").borders.top.is_none() && style_at(&m, "H5").borders.bottom.is_some());
    }

    #[test]
    fn merge_and_unmerge() {
        let mut m = budget();
        apply_format(&mut m, FormatEdit::Merge { range: r("H5:I6") }).unwrap();
        assert!(m.merges.contains(&r("H5:I6")));
        // 기존 병합(B3:B4)과 일부만 겹치면 거절
        assert_eq!(
            apply_format(&mut m, FormatEdit::Merge { range: r("B4:B5") }),
            Err(FormatError::PartialMerge(r("B3:B4")))
        );
        // 값이 있는 칸을 합치면 거절(E3·E4 모두 값)
        assert_eq!(
            apply_format(&mut m, FormatEdit::Merge { range: r("E3:E4") }),
            Err(FormatError::WouldLoseData(CellRef::parse("E4").unwrap()))
        );
        assert_eq!(apply_format(&mut m, FormatEdit::Merge { range: r("H5") }), Err(FormatError::SingleCell));
        apply_format(&mut m, FormatEdit::Unmerge { range: r("I6") }).unwrap();
        assert!(!m.merges.contains(&r("H5:I6")));
    }
}
