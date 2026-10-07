package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType

/**
 * Small GitHub-flavored Markdown renderer for release notes: headings,
 * paragraphs, bullet / numbered lists (nested by indent), task boxes,
 * quotes, fenced code, rules; inline bold, italic, strikethrough, code,
 * links and bare URLs. Anything else reads as plain text.
 */
@Composable
fun MarkdownText(markdown: String, colors: ZeronColors, modifier: Modifier = Modifier) {
    val blocks = remember(markdown) { parseBlocks(markdown) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        blocks.forEach { block ->
            when (block) {
                is Block.Heading -> Text(
                    inline(block.text, colors),
                    color = colors.text,
                    fontFamily = ZeronType.Sans,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = when (block.level) { 1 -> 19.sp; 2 -> 17.sp; else -> 15.5.sp },
                    modifier = Modifier.padding(top = if (block.level <= 2) 6.dp else 2.dp),
                )
                is Block.Paragraph -> Text(inline(block.text, colors), color = colors.text, fontFamily = ZeronType.Sans, fontSize = 14.sp, lineHeight = 20.sp)
                is Block.Item -> Row(Modifier.padding(start = (block.depth * 16).dp)) {
                    Text(
                        when {
                            block.task != null -> if (block.task) "☑" else "☐"
                            block.number != null -> "${block.number}."
                            else -> if (block.depth == 0) "•" else "◦"
                        },
                        color = colors.secondary,
                        fontFamily = ZeronType.Sans,
                        fontSize = 14.sp,
                        lineHeight = 20.sp,
                        modifier = Modifier.width(if (block.number != null) 22.dp else 16.dp),
                    )
                    Text(inline(block.text, colors), color = colors.text, fontFamily = ZeronType.Sans, fontSize = 14.sp, lineHeight = 20.sp)
                }
                is Block.Quote -> Row {
                    Box(Modifier.width(3.dp).height(20.dp).background(colors.hairline))
                    Spacer(Modifier.width(10.dp))
                    Text(inline(block.text, colors), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 14.sp, lineHeight = 20.sp)
                }
                is Block.Code -> Text(
                    block.text,
                    color = colors.text,
                    fontFamily = ZeronType.Mono,
                    fontSize = 12.5.sp,
                    lineHeight = 18.sp,
                    softWrap = false,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .background(colors.codeBackground)
                        .horizontalScroll(rememberScrollState())
                        .padding(10.dp),
                )
                Block.Rule -> HorizontalDivider(color = colors.hairline, modifier = Modifier.padding(vertical = 4.dp))
                is Block.Table -> MarkdownTable(block, colors)
            }
        }
    }
}

private sealed interface Block {
    data class Heading(val level: Int, val text: String) : Block
    data class Paragraph(val text: String) : Block
    data class Item(val depth: Int, val number: Int?, val task: Boolean?, val text: String) : Block
    data class Quote(val text: String) : Block
    data class Code(val text: String) : Block
    data object Rule : Block
    data class Table(val header: List<String>, val aligns: List<TextAlign>, val rows: List<List<String>>) : Block
}

/** Pipe table: hairline frame, tinted header row, per-row separators; scrolls sideways when wider than the screen. */
@Composable
private fun MarkdownTable(t: Block.Table, colors: ZeronColors) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val bodyStyle = TextStyle(fontFamily = ZeronType.Sans, fontSize = 13.sp, lineHeight = 17.sp)
    val headStyle = bodyStyle.copy(fontWeight = FontWeight.SemiBold)
    val padX = 10.dp
    val padY = 7.dp
    val maxCol = 240.dp
    val cols = t.header.size
    // Uniform column widths: widest single-line cell, capped so long cells wrap.
    val colWidths = remember(t, density) {
        (0 until cols).map { c ->
            val texts = listOfNotNull(t.header.getOrNull(c)?.let { it to headStyle }) + t.rows.map { (it.getOrElse(c) { "" }) to bodyStyle }
            val natural = texts.maxOf { (s, st) -> measurer.measure(AnnotatedString(s), style = st, maxLines = 1).size.width }
            with(density) { natural.toDp() }.coerceAtMost(maxCol)
        }
    }
    val radius = RoundedCornerShape(10.dp)
    Box(
        Modifier
            .clip(radius)
            .border(1.dp, colors.hairline, radius)
            .horizontalScroll(rememberScrollState()),
    ) {
        Column {
            Row(Modifier.background(colors.codeBackground)) {
                t.header.forEachIndexed { c, cell ->
                    Text(
                        inline(cell, colors),
                        style = headStyle,
                        color = colors.text,
                        textAlign = t.aligns.getOrElse(c) { TextAlign.Left },
                        modifier = Modifier.width(colWidths[c] + padX * 2).padding(horizontal = padX, vertical = padY),
                    )
                }
            }
            t.rows.forEach { row ->
                HorizontalDivider(color = colors.hairline)
                Row {
                    (0 until cols).forEach { c ->
                        Text(
                            inline(row.getOrElse(c) { "" }, colors),
                            style = bodyStyle,
                            color = colors.text,
                            textAlign = t.aligns.getOrElse(c) { TextAlign.Left },
                            modifier = Modifier.width(colWidths[c] + padX * 2).padding(horizontal = padX, vertical = padY),
                        )
                    }
                }
            }
        }
    }
}

private val headingRe = Regex("^(#{1,6})\\s+(.*?)\\s*#*\\s*$")
private val bulletRe = Regex("^(\\s*)[-*+]\\s+(.*)$")
private val numberRe = Regex("^(\\s*)(\\d{1,3})[.)]\\s+(.*)$")
private val taskRe = Regex("^\\[([ xX])]\\s+(.*)$")
private val ruleRe = Regex("^\\s{0,3}([-*_])(\\s*\\1){2,}\\s*$")
private val tableDelimRe = Regex("^\\s*\\|?\\s*:?-+:?\\s*(\\|\\s*:?-+:?\\s*)+\\|?\\s*$")

/** Splits a pipe-table line into cells; a leading/trailing pipe is optional, `\|` stays literal. */
private fun splitTableRow(line: String): List<String> {
    var s = line.trim()
    if (s.startsWith("|")) s = s.drop(1)
    if (s.endsWith("|") && !s.endsWith("\\|")) s = s.dropLast(1)
    val cells = ArrayList<String>()
    val cur = StringBuilder()
    var i = 0
    while (i < s.length) {
        if (s[i] == '\\' && i + 1 < s.length && s[i + 1] == '|') { cur.append('|'); i += 2 }
        else if (s[i] == '|') { cells.add(cur.toString().trim()); cur.clear(); i++ }
        else { cur.append(s[i]); i++ }
    }
    cells.add(cur.toString().trim())
    return cells
}

private fun tableAlign(cell: String) = when {
    cell.startsWith(":") && cell.endsWith(":") -> TextAlign.Center
    cell.endsWith(":") -> TextAlign.Right
    else -> TextAlign.Left
}

private fun parseBlocks(source: String): List<Block> {
    val out = ArrayList<Block>()
    val para = StringBuilder()
    fun flush() {
        if (para.isNotBlank()) out.add(Block.Paragraph(para.toString().trim()))
        para.clear()
    }
    val lines = source.replace("\r\n", "\n").split('\n')
    var i = 0
    while (i < lines.size) {
        val line = lines[i]
        val trimmed = line.trim()
        when {
            trimmed.startsWith("```") || trimmed.startsWith("~~~") -> {
                flush()
                val fence = trimmed.take(3)
                val code = ArrayList<String>()
                i++
                while (i < lines.size && !lines[i].trim().startsWith(fence)) code.add(lines[i++])
                out.add(Block.Code(code.joinToString("\n")))
            }
            trimmed.isEmpty() -> flush()
            ruleRe.matches(line) -> { flush(); out.add(Block.Rule) }
            headingRe.matches(trimmed) -> {
                flush()
                val m = headingRe.find(trimmed)!!
                out.add(Block.Heading(m.groupValues[1].length, m.groupValues[2]))
            }
            bulletRe.matches(line) -> {
                flush()
                val m = bulletRe.find(line)!!
                val depth = m.groupValues[1].replace("\t", "  ").length / 2
                val task = taskRe.find(m.groupValues[2])
                out.add(Block.Item(depth, null, task?.let { it.groupValues[1] != " " }, task?.groupValues?.get(2) ?: m.groupValues[2]))
            }
            numberRe.matches(line) -> {
                flush()
                val m = numberRe.find(line)!!
                out.add(Block.Item(m.groupValues[1].replace("\t", "  ").length / 2, m.groupValues[2].toInt(), null, m.groupValues[3]))
            }
            trimmed.startsWith(">") -> { flush(); out.add(Block.Quote(trimmed.trimStart('>').trim())) }
            trimmed.contains('|') && i + 1 < lines.size && tableDelimRe.matches(lines[i + 1]) -> {
                flush()
                val header = splitTableRow(line)
                val aligns = splitTableRow(lines[i + 1]).map(::tableAlign)
                val cols = header.size
                val rows = ArrayList<List<String>>()
                i += 2
                while (i < lines.size && lines[i].isNotBlank() && lines[i].contains('|')) {
                    val cells = splitTableRow(lines[i])
                    rows.add(List(cols) { cells.getOrElse(it) { "" } })
                    i++
                }
                i--
                out.add(Block.Table(header, aligns, rows))
            }
            else -> {
                // A continuation line of the previous list item joins it.
                val last = out.lastOrNull()
                if (para.isEmpty() && last is Block.Item && line.startsWith("  ")) {
                    out[out.size - 1] = last.copy(text = last.text + " " + trimmed)
                } else {
                    if (para.isNotEmpty()) para.append(if (line.endsWith("  ")) "\n" else " ")
                    para.append(trimmed)
                }
            }
        }
        i++
    }
    flush()
    return out
}

private val urlRe = Regex("https?://[^\\s)<>]+[^\\s)<>.,;:!?'\"]")

/** Inline spans: `code`, **bold**, *italic* / _italic_, ~~strike~~, [text](url), bare URLs. */
private fun inline(text: String, colors: ZeronColors): AnnotatedString = buildAnnotatedString {
    val link = TextLinkStyles(SpanStyle(color = colors.accent, textDecoration = TextDecoration.Underline))
    var i = 0
    fun find(token: String, from: Int) = text.indexOf(token, from).takeIf { it > from }
    while (i < text.length) {
        val c = text[i]
        val rest = text.substring(i)
        when {
            c == '`' -> {
                val end = text.indexOf('`', i + 1)
                if (end > i) {
                    withStyle(SpanStyle(fontFamily = ZeronType.Mono, background = colors.codeBackground, fontSize = 12.5.sp)) { append(text.substring(i + 1, end)) }
                    i = end + 1
                } else { append(c); i++ }
            }
            rest.startsWith("**") || rest.startsWith("__") -> {
                val end = find(rest.take(2), i + 2)
                if (end != null) {
                    withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append(inline(text.substring(i + 2, end), colors)) }
                    i = end + 2
                } else { append(rest.take(2)); i += 2 }
            }
            rest.startsWith("~~") -> {
                val end = find("~~", i + 2)
                if (end != null) {
                    withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { append(inline(text.substring(i + 2, end), colors)) }
                    i = end + 2
                } else { append("~~"); i += 2 }
            }
            (c == '*' || c == '_') && i + 1 < text.length && !text[i + 1].isWhitespace() && (c == '*' || i == 0 || !text[i - 1].isLetterOrDigit()) -> {
                val end = find(c.toString(), i + 1)
                if (end != null && !text[end - 1].isWhitespace()) {
                    withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(inline(text.substring(i + 1, end), colors)) }
                    i = end + 1
                } else { append(c); i++ }
            }
            c == '[' -> {
                val close = text.indexOf("](", i)
                val end = if (close > i) text.indexOf(')', close) else -1
                if (close > i && end > close) {
                    val url = text.substring(close + 2, end).trim()
                    withLink(LinkAnnotation.Url(url, link)) { append(inline(text.substring(i + 1, close), colors)) }
                    i = end + 1
                } else { append(c); i++ }
            }
            rest.startsWith("http://") || rest.startsWith("https://") -> {
                val m = urlRe.find(text, i)
                if (m != null && m.range.first == i) {
                    withLink(LinkAnnotation.Url(m.value, link)) { append(m.value) }
                    i = m.range.last + 1
                } else { append(c); i++ }
            }
            else -> { append(c); i++ }
        }
    }
}
