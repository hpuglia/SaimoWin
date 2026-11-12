//! Política testável de seleção do DVR Windows. O núcleo de captura nativa
//! será conectado separadamente; não confundir este seletor com uma
//! implementação de Windows.Graphics.Capture.
//!
//! Princípios:
//! - sempre tentar cópia sem recompressão para um fluxo verificável;
//! - escolher captura de janela quando URL HLS disfarçada falha;
//! - preferir H.264 por hardware, com fallback para software;
//! - nunca capturar áudio de outros aplicativos sem informar o usuário;
//! - salvar em MP4 fragmentado e validar saída antes de dizer "gravando".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fonte {
    Comum,
    HlsDisfarcado,
    Protegida,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metodo {
    CopiaDireta,
    JanelaWgc,
    DesktopDxgi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audio {
    SomenteAplicativo,
    SistemaInteiro,
    Indisponivel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    H264Hardware,
    H264Software,
}

#[derive(Clone, Copy, Debug)]
pub struct Capacidades {
    /// Build do Windows (WGC: 18362, áudio por processo: 20348).
    pub windows_build: u32,
    pub video_por_hardware: bool,
    pub decoder_renderizando: bool,
    pub permitir_audio_sistema: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plano {
    pub metodos: Vec<Metodo>,
    pub encoders: Vec<Encoder>,
    pub audio: Audio,
    pub mp4_fragmentado: bool,
    pub avisos: Vec<&'static str>,
}

pub fn escolher(fonte: Fonte, caps: Capacidades) -> Result<Plano, &'static str> {
    if fonte == Fonte::Protegida {
        return Err("Fonte protegida: não tentar contornar proteção de conteúdo");
    }
    let mut metodos = Vec::new();
    let captura_janela = caps.decoder_renderizando && caps.windows_build >= 18362;
    if fonte == Fonte::HlsDisfarcado && captura_janela {
        metodos.push(Metodo::JanelaWgc);
        metodos.push(Metodo::CopiaDireta);
    } else {
        metodos.push(Metodo::CopiaDireta);
        if captura_janela { metodos.push(Metodo::JanelaWgc); }
    }
    if caps.decoder_renderizando && caps.windows_build < 18362 {
        metodos.push(Metodo::DesktopDxgi);
    }
    let audio = if caps.windows_build >= 20348 {
        Audio::SomenteAplicativo
    } else if caps.permitir_audio_sistema {
        Audio::SistemaInteiro
    } else {
        Audio::Indisponivel
    };
    let mut avisos = Vec::new();
    if audio == Audio::Indisponivel {
        avisos.push("O Windows não suporta captura de áudio por processo e a captura de áudio geral não foi autorizada");
    }
    if audio == Audio::SistemaInteiro {
        avisos.push("A captura de áudio geral pode incluir o som de outros programas");
    }
    if !caps.decoder_renderizando {
        avisos.push("A captura de janela só funciona se o player estiver renderizando o canal");
    }
    Ok(Plano {
        metodos,
        encoders: if caps.video_por_hardware {
            vec![Encoder::H264Hardware, Encoder::H264Software]
        } else {
            vec![Encoder::H264Software]
        },
        audio,
        mp4_fragmentado: true,
        avisos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc_atual() -> Capacidades {
        Capacidades {
            windows_build: 26100,
            video_por_hardware: true,
            decoder_renderizando: true,
            permitir_audio_sistema: false,
        }
    }

    #[test]
    fn hls_disfarcado_prefere_captura_e_fallback_direto() {
        let p = escolher(Fonte::HlsDisfarcado, pc_atual()).unwrap();
        assert_eq!(p.metodos, vec![Metodo::JanelaWgc, Metodo::CopiaDireta]);
        assert!(p.mp4_fragmentado);
        assert_eq!(p.audio, Audio::SomenteAplicativo);
    }

    #[test]
    fn url_normal_prefere_copia_sem_recompressao() {
        let p = escolher(Fonte::Comum, pc_atual()).unwrap();
        assert_eq!(p.metodos[0], Metodo::CopiaDireta);
        assert_eq!(p.encoders, vec![Encoder::H264Hardware, Encoder::H264Software]);
    }

    #[test]
    fn pc_fraco_prioriza_software_quando_nao_ha_encoder() {
        let mut caps = pc_atual();
        caps.video_por_hardware = false;
        assert_eq!(escolher(Fonte::HlsDisfarcado, caps).unwrap().encoders, vec![Encoder::H264Software]);
    }

    #[test]
    fn win_antigo_nao_ativa_audio_de_terceiros_sem_consentimento() {
        let caps = Capacidades {
            windows_build: 19045,
            video_por_hardware: false,
            decoder_renderizando: true,
            permitir_audio_sistema: false,
        };
        let p = escolher(Fonte::HlsDisfarcado, caps).unwrap();
        assert_eq!(p.audio, Audio::Indisponivel);
    }

    #[test]
    fn win_antigo_com_permissao_explicitada() {
        let mut caps = pc_atual();
        caps.windows_build = 19045;
        caps.permitir_audio_sistema = true;
        assert_eq!(escolher(Fonte::Comum, caps).unwrap().audio, Audio::SistemaInteiro);
    }

    #[test]
    fn fonte_protegida_recusada() {
        assert!(escolher(Fonte::Protegida, pc_atual()).is_err());
    }
}
