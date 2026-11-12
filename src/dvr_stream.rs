//! Gravação direta de TV: usa cópia de pacotes FFmpeg em arquivos Matroska
//! independentes (30 s por trecho) sem OBS, reencodificação ou captura de tela.
//! Os trechos permanecem no disco mesmo que o programa encerre inesperadamente.
use crate::catalogo;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TEMPO_SEGMENTO: u32 = 30;
const EXT_HLS: &str = "3gp,aac,ac3,eac3,ec3,flac,fmp4,m3u8,m4a,m4s,mp3,mp4,mpeg,mpegts,mkv,mov,ts,txt,vtt,webvtt,cmfv,cmfa,woff";
const MIN_BYTES: u64 = 1024;

fn e_playlist_texto(url: &str) -> bool {
    url.split('?').next().unwrap_or(url).to_ascii_lowercase().ends_with(".txt")
}

fn e_playlist_hls(url: &str) -> bool {
    let sem_query = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    sem_query.ends_with(".m3u8") || sem_query.ends_with(".txt")
}

fn args_hls(url: &str, tentativa: u8) -> Vec<String> {
    let mut args = Vec::new();
    if e_playlist_hls(url) && tentativa > 0 {
        // O HLS de alguns canais usa segmentos MPEG-TS com extensão .woff.
        // O FFmpeg moderno aplica dois filtros de extensões; ambos precisam
        // da lista explícita. A checagem formato x extensão também rejeita
        // MPEG-TS válido com nome .woff, portanto o fallback a desativa
        // SOMENTE na segunda tentativa de uma playlist HLS identificada,
        // mantendo uma lista restrita e protocolos de rede limitados.
        // Nunca usar ALL nem aplicar esse ajuste globalmente.
        args.extend([
            "-allowed_extensions".into(), EXT_HLS.into(),
            "-allowed_segment_extensions".into(), EXT_HLS.into(),
            "-extension_picky".into(), "0".into(),
            "-protocol_whitelist".into(), "http,https,tcp,tls,crypto".into(),
        ]);
    }
    if e_playlist_texto(url) {
        // Igual ao demuxer-lavf-format=hls que o Saimo já usa no mpv.
        args.extend(["-f".into(), "hls".into()]);
    }
    args
}

fn destino_base(nome: &str) -> String {
    let n: String = nome.chars().map(|c| {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }
    }).take(60).collect();
    let n = n.trim().trim_matches('.');
    if n.is_empty() { "Canal".into() } else { n.into() }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Estado {
    canal: String,
    inicio: String,
    estado: String,
    metodo: String,
    bytes: u64,
    partes: usize,
    erro: Option<String>,
}

fn salvar_estado(pasta: &Path, estado: &Estado) {
    let Ok(texto) = serde_json::to_vec_pretty(estado) else { return };
    let temporario = pasta.join("sessao.json.tmp");
    if fs::write(&temporario, texto).is_ok() {
        let _ = fs::rename(temporario, pasta.join("sessao.json"));
    }
}

fn ler_estado(pasta: &Path) -> Option<Estado> {
    fs::read(pasta.join("sessao.json")).ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn partes_da_sessao(pasta: &Path) -> Vec<PathBuf> {
    let Ok(itens) = fs::read_dir(pasta) else { return Vec::new() };
    let mut arquivos: Vec<PathBuf> = itens.flatten().map(|i| i.path()).filter(|p| {
        let nome = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        nome.starts_with("parte_") && nome.ends_with(".mkv")
            && fs::metadata(p).is_ok_and(|m| m.len() >= MIN_BYTES)
    }).collect();
    arquivos.sort();
    arquivos
}

fn bytes_da_sessao(pasta: &Path) -> (u64, usize) {
    let partes = partes_da_sessao(pasta);
    let bytes = partes.iter().map(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum();
    (bytes, partes.len())
}

fn nome_da_sessao(canal: &str, raiz: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(raiz).map_err(|e| format!("Pasta de gravações: {e}"))?;
    let base = destino_base(canal);
    for n in 0..1000 {
        let data = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let sufixo = if n == 0 { String::new() } else { format!("-{n}") };
        let pasta = raiz.join(format!("{base}_{data}{sufixo}"));
        if !pasta.exists() {
            fs::create_dir(&pasta).map_err(|e| format!("Criar pasta da gravação: {e}"))?;
            return Ok(pasta);
        }
    }
    Err("Não foi possível reservar uma pasta exclusiva de gravação".into())
}

fn caminho_local_ffconcat(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

fn validar_fluxos(ffmpeg: &Path, arquivo: &Path) -> Option<(bool, bool)> {
    let probe = if ffmpeg.is_absolute() {
        ffmpeg.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" })
    } else {
        PathBuf::from(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" })
    };
    let resposta = Command::new(probe)
        .args(["-v", "error", "-show_entries", "stream=codec_type",
            "-of", "csv=p=0"])
        .arg(arquivo).stdout(Stdio::piped()).stderr(Stdio::null())
        .output().ok()?;
    if !resposta.status.success() { return None; }
    let saida = String::from_utf8_lossy(&resposta.stdout);
    Some((
        saida.lines().any(|s| s.trim() == "video"),
        saida.lines().any(|s| s.trim() == "audio"),
    ))
}

fn finalizar_em_segundo_plano(pasta: PathBuf, ffmpeg: PathBuf, recuperar: bool) {
    std::thread::spawn(move || {
        if recuperar {
            // Se o Saimo caiu mas o filho FFmpeg ainda está finalizando, esperar
            // os arquivos estabilizarem; nunca unir arquivos em escrita.
            for _ in 0..5 {
                let antes = bytes_da_sessao(&pasta);
                std::thread::sleep(Duration::from_secs(15));
                if bytes_da_sessao(&pasta) == antes { break; }
            }
        }
        let partes = partes_da_sessao(&pasta);
        if partes.is_empty() { return; }
        let mut estado = ler_estado(&pasta).unwrap_or_else(|| Estado {
            canal: "Gravação recuperada".into(), inicio: String::new(),
            estado: "interrompida".into(), metodo: "ffmpeg -c copy".into(),
            bytes: 0, partes: 0, erro: None,
        });
        let lista = pasta.join("partes.ffconcat");
        let mut entradas = String::from("ffconcat version 1.0\n");
        for arquivo in &partes {
            entradas.push_str("file '");
            entradas.push_str(&caminho_local_ffconcat(arquivo).replace('\'', "'\\''"));
            entradas.push_str("'\n");
        }
        if fs::write(&lista, entradas).is_err() { return; }
        let parcial = pasta.join("gravacao_final.tmp.mkv");
        let finalizado = pasta.join("gravacao_final.mkv");
        if finalizado.exists() { return; }
        let ok = Command::new(&ffmpeg)
            .current_dir(&pasta)
            .args(["-hide_banner", "-nostats", "-loglevel", "error", "-y",
                "-f", "concat", "-safe", "0", "-i"])
            .arg(&lista)
            .args(["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy", "-f", "matroska"])
            .arg(&parcial)
            .stdin(Stdio::null())
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().is_ok_and(|s| s.success());
        if ok && fs::metadata(&parcial).is_ok_and(|m| m.len() >= MIN_BYTES)
            && fs::rename(&parcial, &finalizado).is_ok() {
            let (bytes, qtd) = bytes_da_sessao(&pasta);
            estado.bytes = bytes;
            estado.partes = qtd;
            match validar_fluxos(&ffmpeg, &finalizado) {
                Some((true, true)) => estado.estado = "finalizada".into(),
                Some((true, false)) => {
                    estado.estado = "finalizada_sem_audio".into();
                    estado.erro = Some("O arquivo possui vídeo, mas nenhuma faixa de áudio identificada".into());
                }
                Some((false, _)) => {
                    estado.estado = "verificar_arquivo".into();
                    estado.erro = Some("Não foi identificada uma faixa de vídeo na gravação final".into());
                }
                None => {
                    estado.estado = "finalizada_nao_verificada".into();
                    estado.erro = Some("Instale ffprobe junto do ffmpeg para validar vídeo e áudio".into());
                }
            }
            salvar_estado(&pasta, &estado);
        } else {
            let _ = fs::remove_file(parcial);
            estado.estado = "partes_recuperaveis".into();
            estado.erro = Some("A união falhou. Os trechos MKV continuam disponíveis".into());
            salvar_estado(&pasta, &estado);
        }
    });
}

/// Reabrir o aplicativo não destrói sessões interrompidas. Tenta montar
/// o arquivo final em segundo plano, mantendo todos os trechos originais.
pub fn recuperar_sessoes(raiz: &Path, ffmpeg: &Path) -> usize {
    let Ok(itens) = fs::read_dir(raiz) else { return 0 };
    let mut recuperadas = 0;
    for dir in itens.flatten().filter(|i| i.path().is_dir()) {
        let pasta = dir.path();
        let Some(mut estado) = ler_estado(&pasta) else { continue };
        if estado.estado == "gravando" || estado.estado == "iniciando"
            || estado.estado == "finalizando" {
            estado.estado = "interrompida".into();
            let (bytes, n) = bytes_da_sessao(&pasta);
            estado.bytes = bytes;
            estado.partes = n;
            salvar_estado(&pasta, &estado);
            if n > 0 {
                finalizar_em_segundo_plano(pasta, ffmpeg.to_path_buf(), true);
                recuperadas += 1;
            }
        }
    }
    recuperadas
}

pub struct GravacaoStream {
    pub canal: String,
    pub pasta: PathBuf,
    pub log: PathBuf,
    filho: Child,
    ffmpeg: PathBuf,
    fonte: catalogo::Fonte,
    tentativa: u8,
    limite: Option<u64>,
    iniciada: Instant,
    ultima_verificacao: Instant,
    ultimo_bytes: u64,
    ultimas_partes: usize,
    ultima_contagem: Instant,
    ultima_evolucao: Instant,
    pub parada_solicitada: bool,
    parar_em: Option<Instant>,
    houve_dados: bool,
}

impl GravacaoStream {
    pub fn iniciar(
        canal: &catalogo::Canal, indice: usize, duracao: Option<u64>,
        ffmpeg: PathBuf, raiz: &Path,
    ) -> Result<Self, String> {
        let fonte = canal.fontes.get(indice).ok_or("Fonte não encontrada")?.clone();
        if fonte.chave.is_some() { return Err("A fonte informa proteção de conteúdo".into()); }
        if !fonte.url.starts_with("https://") && !fonte.url.starts_with("http://") {
            return Err("Gravação direta só permite transmissões HTTP(S)".into());
        }
        let pasta = nome_da_sessao(&canal.nome, raiz)?;
        let estado = Estado {
            canal: canal.nome.clone(), inicio: Local::now().to_rfc3339(),
            estado: "iniciando".into(), metodo: "ffmpeg -c copy".into(),
            bytes: 0, partes: 0, erro: None,
        };
        salvar_estado(&pasta, &estado);
        let (filho, log) = Self::abrir_ffmpeg(&ffmpeg, &pasta, &fonte, duracao, 0)?;
        Ok(Self {
            canal: canal.nome.clone(), pasta, log, filho, ffmpeg, fonte,
            tentativa: 0, limite: duracao, iniciada: Instant::now(),
            ultima_verificacao: Instant::now(),
            ultimo_bytes: 0, ultimas_partes: 0,
            ultima_contagem: Instant::now(), ultima_evolucao: Instant::now(),
            parada_solicitada: false, parar_em: None, houve_dados: false,
        })
    }

    fn abrir_ffmpeg(
        ffmpeg: &Path, pasta: &Path, fonte: &catalogo::Fonte,
        duracao: Option<u64>, tentativa: u8,
    ) -> Result<(Child, PathBuf), String> {
        let log = pasta.join(format!("tentativa_{}.ffmpeg.log", tentativa + 1));
        let saida_log = File::create(&log)
            .map_err(|e| format!("Não foi possível escrever log: {e}"))?;
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "warning", "-nostats", "-y"]);
        cmd.args(["-user_agent", fonte.agente.as_deref().unwrap_or(crate::AGENTE)]);
        if let Some(referer) = fonte.referer.as_deref() {
            cmd.args(["-referer", referer]);
        }
        cmd.args(["-rw_timeout", "15000000",
            "-reconnect", "1", "-reconnect_streamed", "1",
            "-reconnect_on_network_error", "1", "-reconnect_delay_max", "5"]);
        cmd.args(args_hls(&fonte.url, tentativa));
        cmd.arg("-i").arg(&fonte.url)
            .args(["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy",
                "-f", "segment", "-segment_time", &TEMPO_SEGMENTO.to_string(),
                "-reset_timestamps", "1", "-segment_format", "matroska",
                "-segment_list_type", "ffconcat", "-segment_list"])
            .arg(pasta.join("partes.ffconcat"));
        if let Some(secs) = duracao {
            // -t como opção de saída, não como input.
            cmd.args(["-t", &secs.max(1).to_string()]);
        }
        cmd.arg(pasta.join("parte_%05d.mkv"))
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(saida_log));
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let filho = cmd.spawn().map_err(|e| format!("Falha ao iniciar FFmpeg: {e}"))?;
        Ok((filho, log))
    }

    pub fn solicitar_parada(&mut self) -> Result<(), String> {
        if self.parada_solicitada { return Ok(()); }
        self.parada_solicitada = true;
        self.parar_em = Some(Instant::now());
        if let Some(mut stdin) = self.filho.stdin.take() {
            stdin.write_all(b"q\n").map_err(|e| format!("FFmpeg: {e}"))?;
        }
        Ok(())
    }

    /// Retorna (ainda_ativa, avisos). A sessão finaliza em background e
    /// todos os arquivos independentes ficam disponíveis se a união falhar.
    pub fn atualizar(&mut self) -> (bool, Vec<String>) {
        let mut avisos = Vec::new();
        // Numa gravação longa podem existir centenas de trechos. Não percorrer
        // o diretório a cada frame/segundo em máquinas com HDD lento.
        let (mut bytes, mut n) = if self.ultima_contagem.elapsed() >= Duration::from_secs(3)
            || self.ultimo_bytes == 0 || self.parada_solicitada {
            self.ultima_contagem = Instant::now();
            let (bytes, n) = bytes_da_sessao(&self.pasta);
            self.ultimas_partes = n;
            (bytes, n)
        } else {
            (self.ultimo_bytes, self.ultimas_partes)
        };
        if bytes > 0 && !self.houve_dados {
            self.houve_dados = true;
            avisos.push(format!(
                "DVR: gravação confirmada de {} ({} bytes; {} trecho(s))",
                self.canal, bytes, n
            ));
        }
        if bytes != self.ultimo_bytes {
            self.ultimo_bytes = bytes;
            self.ultima_evolucao = Instant::now();
        }
        if self.ultima_verificacao.elapsed() >= Duration::from_secs(10) {
            self.ultima_verificacao = Instant::now();
            if let Some(mut estado) = ler_estado(&self.pasta) {
                estado.estado = if self.parada_solicitada { "finalizando" } else { "gravando" }.into();
                estado.bytes = bytes;
                estado.partes = n;
                salvar_estado(&self.pasta, &estado);
            }
        }
        if self.parada_solicitada && self.parar_em.is_some_and(|p| p.elapsed() >= Duration::from_secs(12)) {
            let _ = self.filho.kill();
            avisos.push(format!(
                "DVR: encerramento demorado de {}. Preservando os trechos gravados.",
                self.canal
            ));
        }
        if !self.parada_solicitada && self.ultima_evolucao.elapsed() >= Duration::from_secs(35) {
            let _ = self.filho.kill();
            avisos.push(format!(
                "DVR: sem novos dados em {} há 35 segundos; encerrando a tentativa para preservar os arquivos.",
                self.canal
            ));
        }
        match self.filho.try_wait() {
            Ok(None) => (true, avisos),
            Ok(Some(status)) => {
                // Nunca usar dados em cache para decidir se a gravação falhou.
                (bytes, n) = bytes_da_sessao(&self.pasta);
                if bytes == 0 && !self.parada_solicitada && self.tentativa == 0
                    && e_playlist_hls(&self.fonte.url) {
                    self.tentativa = 1;
                    let restante = self.limite.map(|total|
                        total.saturating_sub(self.iniciada.elapsed().as_secs()).max(1)
                    );
                    match Self::abrir_ffmpeg(&self.ffmpeg, &self.pasta, &self.fonte, restante, 1) {
                        Ok((novo, log)) => {
                            self.filho = novo;
                            self.log = log;
                            self.ultima_evolucao = Instant::now();
                            avisos.push(format!(
                                "DVR: primeira leitura falhou; tentando extensão HLS alternativa para {}",
                                self.canal
                            ));
                            return (true, avisos);
                        }
                        Err(e) => avisos.push(format!("DVR: alternativa indisponível: {e}")),
                    }
                }
                if let Some(mut estado) = ler_estado(&self.pasta) {
                    estado.bytes = bytes;
                    estado.partes = n;
                    estado.estado = if bytes > 0 { "finalizando" } else { "falhou" }.into();
                    if bytes == 0 {
                        estado.erro = Some(format!(
                            "Nenhum pacote de vídeo recebido; saída do FFmpeg: {:?}. Consulte o log local.",
                            status.code()
                        ));
                    }
                    salvar_estado(&self.pasta, &estado);
                }
                if bytes == 0 {
                    avisos.push(format!(
                        "DVR: {} terminou sem vídeo. Consulte {}",
                        self.canal, self.log.display()
                    ));
                } else {
                    finalizar_em_segundo_plano(self.pasta.clone(), self.ffmpeg.clone(), false);
                    avisos.push(format!(
                        "DVR: {} — {} trecho(s) salvos; preparando arquivo final em segundo plano.",
                        self.canal, n
                    ));
                }
                (false, avisos)
            }
            Err(e) => {
                avisos.push(format!("DVR: erro de acompanhamento do FFmpeg: {e}"));
                (false, avisos)
            }
        }
    }
}

impl Drop for GravacaoStream {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.filho.stdin.take() {
            let _ = stdin.write_all(b"q\n");
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn hls_txt_tem_demuxer_forcado_e_nao_desliga_filtros() {
        assert_eq!(args_hls("https://exemplo.test/live.txt?x=123", 0),
            vec!["-f".to_string(), "hls".to_string()]);
        let alternativo = args_hls("https://exemplo.test/live.txt", 1);
        assert!(alternativo.contains(&"-allowed_extensions".to_string()));
        assert!(alternativo.contains(&"-allowed_segment_extensions".to_string()));
        assert!(alternativo.windows(2).any(|p| p == ["-extension_picky", "0"]));
        assert!(!alternativo.contains(&"ALL".to_string()));
    }

    #[test]
    fn URL_m3u8_faz_seleção_normal_primeiro() {
        assert!(args_hls("https://exemplo.test/live.m3u8", 0).is_empty());
        assert!(!e_playlist_hls("https://exemplo.test/file.mp4"));
    }

    #[test]
    fn cada_trecho_possui_nome_independente() {
        assert_eq!(destino_base("SBT: São Paulo / HD"), "SBT_ São Paulo _ HD");
    }
}
