# Laurel: guia do teste com amigos

O Laurel guarda os últimos segundos do jogo que você está jogando e salva um clipe quando você aperta uma tecla. Ele acha seus jogos sozinho, grava o som do jogo separado do microfone e deixa tudo no seu PC: nada é enviado para lugar nenhum.

Esta é uma versão de teste. Ela pode ter erros, e é exatamente isso que queremos achar com a sua ajuda.

## O que mudou na 0.1.0-rc.6

- **Som do jogo nos clipes.** Quando o Laurel não conseguia entrar no jogo logo que ele abria, ele passava a gravar a janela do jogo (com o aviso "Gravando {jogo} pela janela"), mas deixava o som do jogo de fora. Com **Jogo e microfone**, o clipe ficava só com a sua voz; com **Só o jogo**, ficava mudo. Isso acontecia muito no Overwatch. Agora o som do jogo entra também nesse caso. Os clipes que já saíram sem o som do jogo não têm como recuperá-lo.

## O que mudou na 0.1.0-rc.5

Esta versão prepara os **testes de estabilidade**: jogue normalmente e, depois de algumas sessões, mande os logs.

- **Salvar logs para enviar.** Em **Configurações > Avançado**, o botão cria um único `.zip` na pasta **Downloads** e abre a pasta com ele selecionado. É esse arquivo que você manda. Ele traz as cinco últimas sessões do Laurel e o log do motor de gravação, agora com a hora de cada linha, e o seu nome de usuário do Windows fica escondido.
- **Um Laurel por vez.** Abrir o Laurel de novo pelo menu Iniciar, com ele já esperando na bandeja, abria uma segunda cópia que gravava o mesmo jogo outra vez. Agora isso só mostra a janela do Laurel que já está aberto.

## O que mudou na 0.1.0-rc.4

- **Clipes das conquistas da Steam.** Enquanto você joga um jogo da Steam, o Laurel salva um clipe sozinho quando você desbloqueia uma conquista rara: menos de 10% dos jogadores (**Rara**) ou menos de 5% (**Ultra rara**). O clipe sai uns 15 segundos depois da conquista, para mostrar o que veio em seguida, e leva o nome dela, por exemplo `Elsie 2026-10-26 12-48-50 - Hi Mom!.mp4`. Toca um som próprio, três notas subindo.
- **Opções** em **Configurações > Clipes automáticos**: ligar e desligar tudo, o som, as três opções (**Ultra rara**, **Rara** e **Qualquer conquista**, que vem desligada) e uma chave para cada jogo da Steam, para deixar um jogo de fora.
- **Na biblioteca**, os clipes de conquista têm um **troféu azul** na miniatura (passe o mouse para ver a conquista e quantos jogadores a têm), o filtro **Conquistas** na lateral e a linha **Conquista** nos detalhes.
- **Como funciona.** Enquanto o jogo roda, um programa pequeno do Laurel lê as suas conquistas pela própria Steam. Ele não mexe no jogo e fecha junto com ele. A raridade vem da Valve, numa consulta por jogo.

## O que mudou na 0.1.0-rc.3

- **Avisos no jogo desligados por padrão.** Medimos que, enquanto o aviso **Clipe salvo** aparece por cima do jogo, o Windows deixa de mostrar a imagem do jogo pelo caminho mais rápido, e alguns jogos podem ter uma leve instabilidade. Por isso, o aviso visual agora vem desligado, e o som de clipe salvo continua. A atualização desliga o aviso uma vez, mesmo para quem já tinha o Laurel.
- **Avisos mais curtos.** Para quem ligar os avisos em **Configurações > Notificações > Mostrar avisos sobre os jogos**, o aviso de clipe e o de captura de tela agora ficam só 1,5 segundo na tela. Ao ligar, aparece em amarelo o lembrete de que alguns jogos podem ter uma leve instabilidade.

## O que mudou na 0.1.0-rc.2

- **Português.** O Laurel abre no idioma do Windows: português ou inglês. Para trocar, vá em **Configurações > Idioma**.
- **Vários clipes de uma vez.** Na biblioteca, selecione com **Ctrl+clique**, com o **clique direito** ou pela caixinha no canto do clipe; **Shift+clique** pega um intervalo, e **Ctrl+A** pega tudo. A barra que aparece em cima mantém, exclui ou cancela a seleção.
- **Excluídos recentemente.** Excluir não manda mais o clipe para a Lixeira do Windows: ele vai para **Excluídos**, na lateral da biblioteca, e fica lá 7 dias antes de sair de vez. Logo depois de excluir aparece **Desfazer**, e dentro de Excluídos dá para **Restaurar** ou **Excluir agora**. O prazo (1, 7, 15 ou 30 dias) fica em **Configurações > Armazenamento**.
- **Clipes de 30 segundos inteiros.** Antes, alguns clipes saíam com 29 e pouco; agora têm pelo menos o tempo escolhido.
- **Logs mais úteis.** Os logs registram o que pediu cada clipe (o atalho, a bandeja ou o botão), para entendermos casos de clipe salvo sem ninguém apertar o F8.

Quem já tem a versão anterior recebe a atualização pelo próprio Laurel: aparece o aviso de versão nova, e é só clicar em **Instalar e reiniciar** (na 0.1.0-rc.1, que ainda era em inglês, **Install and restart**).

## Antes de instalar

- Windows 10 (versão 2004 ou mais nova) ou Windows 11, 64 bits.
- Não precisa desinstalar o OBS ou outro gravador de clipes, mas anote quais você tem: isso ajuda a entender o que acontecer.

## Instalar

1. Baixe o `Laurel_..._x64-setup.exe` da página da versão de teste e abra.
2. O instalador ainda não é assinado, então o Windows pode mostrar **"O Windows protegeu o computador"**. Clique em **Mais informações** e depois em **Executar assim mesmo**.
3. O Laurel é instalado só para o seu usuário, sem pedir administrador. A exceção é quando falta no PC o **Microsoft Visual C++ Runtime**, de que o Laurel precisa para gravar: aí o Windows pede permissão para instalá-lo.
4. Na primeira vez que o Laurel abrir, ele avisa que vai iniciar junto com o Windows (**O Laurel inicia com o Windows**). Escolha **Manter ligado** para ele ficar esperando na bandeja, pronto para gravar, ou **Desligar**.

## Usar

- Abra um jogo. O Laurel acha os jogos da Steam, Epic, Riot e Xbox e começa a gravar sozinho; no topo da janela aparece **Gravando** com o nome do jogo.
- **F8** salva um clipe com os últimos 30 segundos. **Shift+F8** tira uma captura de tela. As teclas mudam em **Configurações > Atalhos**.
- Os clipes ficam na pasta **Vídeos\Laurel** e aparecem na biblioteca do Laurel, onde dá para assistir, cortar, manter e excluir.
- Se um jogo não for encontrado, adicione em **Configurações > Jogos > Seus jogos**.
- Se o jogo roda como administrador, o Laurel oferece **Reiniciar como administrador** para conseguir gravá-lo.
- Em **Configurações > Gravação** ficam a duração do clipe, a resolução, o fps, o codificador e o som do clipe (**Som do clipe**: só o jogo; jogo e microfone; ou todo o som do PC e microfone).

## O que testar

Não precisa fazer tudo. O que você conseguir já ajuda muito.

1. **Instalação ou atualização:** instalou ou atualizou sem erro? Pediu administrador? Seu PC tinha OBS instalado?
2. **Jogo encontrado:** abriu um jogo e o Laurel mostrou **Gravando** com o nome certo?
3. **Clipe:** o F8 salvou o clipe? Tocou o som de clipe salvo? O clipe tem imagem e som, e pelo menos 30 segundos?
4. **Som:** com o padrão (jogo e microfone), o Discord e a música ficaram fora do clipe? Teste também as outras opções de **Som do clipe**. Se aparecer o aviso **Gravando {jogo} pela janela**, confira se o clipe tem o som do jogo.
5. **Placa de vídeo:** em **Configurações > Gravação**, o item **Gravando agora** mostra o codificador em uso. Diga qual apareceu e qual é a sua placa (NVIDIA, AMD ou Intel). Testar em placas AMD e Intel é muito importante.
6. **Desempenho:** sentiu queda de FPS ou travadas, principalmente quando o jogo abriu ou quando salvou um clipe? Se quiser, ligue **Configurações > Notificações > Mostrar avisos sobre os jogos** e diga se sentiu alguma diferença ao salvar com o aviso na tela. Diga também se o seu monitor usa G-Sync ou FreeSync.
7. **Português:** o Laurel abriu em português? Algum texto ficou estranho, cortado ou em inglês? Mande uma captura de tela.
8. **Seleção e Excluídos:** excluir vários clipes de uma vez funcionou? O **Desfazer** trouxe de volta? **Restaurar** devolveu o clipe para a biblioteca?
9. **Corte:** cortar um clipe na biblioteca funcionou?
10. **Iniciar com o Windows:** depois de reiniciar o PC, o Laurel abriu sozinho na bandeja e gravou o jogo?
11. **Atualização:** quando sair uma versão de teste nova, o Laurel vai oferecer a atualização sozinho. Diga se funcionou.
12. **Conquistas da Steam:** jogando um jogo da Steam, desbloqueie uma conquista. Se ela for comum, ligue antes **Configurações > Clipes automáticos > Qualquer conquista**. O clipe apareceu uns 15 segundos depois, com o troféu na biblioteca? Tocou o som? Em **Clipes automáticos**, a linha de estado mostrou o jogo e as conquistas? Em jogos com anti-cheat, notou algo diferente? Mande o **app.log** se algo não sair.

## Como relatar um problema

Mande pelo nosso canal de sempre:

- o que você fez, o que esperava e o que aconteceu;
- o jogo, a sua placa de vídeo e a versão do Windows;
- os logs: em **Configurações > Avançado**, clique em **Salvar logs para enviar**. O Laurel cria um único arquivo `.zip` na pasta **Downloads** e abre a pasta com ele selecionado; é só mandar esse arquivo. Ele traz as cinco últimas sessões do Laurel, o log do motor de gravação com a hora de cada linha e as versões do Laurel e do Windows, com o seu nome de usuário do Windows escondido.

## Privacidade

Os clipes e as configurações ficam só no seu PC. O Laurel não tem conta, anúncios nem telemetria. A única coisa que ele busca na internet é se existe uma versão nova, e isso pode ser desligado em **Configurações > Sobre > Verificar atualizações**. A atualização só é instalada quando você clica em **Instalar e reiniciar**.

## Desinstalar

Em **Configurações do Windows > Aplicativos > Aplicativos instalados**, procure **Laurel** e desinstale. Os seus clipes em **Vídeos\Laurel** nunca são apagados, nem os que estiverem em Excluídos. Marque **Remover dados do programa** (no desinstalador em inglês, **Delete the application data**) só se quiser apagar também as configurações e o índice da biblioteca.
