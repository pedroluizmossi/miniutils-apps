# Repositórios (app externo do MiniUtils)

Os seus repositórios Git e o que falta em cada um: alterações não commitadas, commits não
enviados (↑), commits do remoto que você ainda não tem (↓), stash e merge/rebase pela
metade. Cada linha tem botões para abrir um terminal, a pasta ou o editor (VS Code,
VSCodium ou Zed, se houver) no repositório.

**Acesso total:** é um programa próprio, que roda com as permissões do seu usuário. Ele lê
as pastas vigiadas e roda o `git` (e o `gh`, se houver) em cada repositório. Por conta
própria, só escreve na pasta de dados dele (`pastas.txt` e `preferencias.json`). No
repositório, só mexe quando você clica: enviar, puxar ou apagar branches.

## Na linha de cada repositório

- **↑ Enviar** aparece com commits à frente do upstream: um `git push`.
- **↓ Puxar** aparece quando o remoto tem commits novos e você não tem nada local (nem
  alteração, nem commit à frente): um `git pull --ff-only`, que só avança a branch e nunca
  cria merge. Sem chave ou senha disponível, falha sem perguntar, e o motivo vem numa
  notificação.
- **Agente de IA:** "Claude trabalhando", "Codex espera você". Vem do provedor `agents` do
  hub (o mesmo do app Agentes de IA: Claude Code, Codex e OpenCode), pela pasta de cada
  sessão. Enquanto um agente está ativo num repositório, as pendências dele não contam como
  velhas. Quando o agente termina deixando alteração sem commit, a linha mostra
  "terminou · sem commit" por até 30 min, e a mascote comenta na conversa.
- **CI ✓ / ✗ / …:** a última execução do CI da branch atual, pelo `gh` (GitHub CLI), só em
  repositório com remoto no github.com e só com o `gh` instalado e com login
  (`gh auth login`). Também mostra "CI de main falhou" quando você está noutra branch, e
  "2 PRs esperam sua revisão". É lido a cada 30 min (a API do GitHub tem cota) e 1 min
  depois de um push pelo botão. Para desligar: opção "Mostrar CI e PRs do GitHub".

## Detalhe

Clique no nome do repositório para abrir: os arquivos alterados (`M` alterado, `A` novo,
`D` apagado, `R` renomeado, `U` em conflito, `?` fora do git), os últimos 3 commits, o
stash e as branches locais já mergeadas na principal. Ali ficam também:

- **Fixar no topo:** o repositório aparece primeiro, mesmo limpo (e mesmo com "Mostrar só
  repositórios com pendências").
- **Ocultar:** some da lista, da mascote e do lembrete. No rodapé, "N ocultos · mostrar"
  traz de volta.
- **Apagar branches mergeadas:** pede um segundo clique e usa `git branch -d`, que recusa
  apagar uma branch que não foi mergeada. Só as locais; a principal e a atual nunca entram.

## A mascote (polvo)

- **Alarme:** conflito, merge/rebase/cherry-pick pela metade, ou CI da branch principal
  falhando.
- **Agitada:** alguma pendência (não commitado ou não enviado) mais velha que o limite da
  opção "Horas até a mascote cobrar pendências" (padrão: 24 h).
- **Quieta:** pendências recentes; você (ou um agente) está trabalhando.
- **Contente:** tudo commitado e enviado.

Na conversa entre mascotes, o app conta quando um repositório fica com pendência velha, em
conflito ou com o CI quebrado, quando um agente termina deixando alteração sem commit, e
quando tudo volta a ficar em dia.

## Lembrete do fim do dia

A partir da hora da opção "Hora do lembrete" (padrão: 18 h), uma notificação por dia se
houver commit sem enviar: "Você tem 4 commits sem enviar em 2 repositórios". Repositório
oculto não conta. Desliga na opção "Lembrar no fim do dia dos commits sem enviar".

## Pastas vigiadas

Na primeira vez, o app cria `pastas.txt` na pasta de dados dele, com as pastas de projetos
que achar na sua pasta pessoal (`~/Projetos`, `~/projects`, `~/src`, `~/code`…).

- **+ Pasta** abre o seletor de pasta do sistema e acrescenta a escolhida (no Linux, precisa
  do `zenity`, do `kdialog` ou do `yad`; no Windows, é o do próprio sistema).
- **Pastas** abre o arquivo para editar à mão: uma pasta por linha, `~` é a pasta pessoal.
  Salve e clique em **Atualizar**.

A busca desce até a profundidade da opção "Profundidade da busca nas pastas" (padrão: 2) e
não entra em pastas ocultas, `node_modules`, `target` e afins.

## Quando atualiza

- **Na hora:** commit, add, checkout, pull, fetch, ou qualquer coisa que mexa dentro do
  `.git`. O app vigia só o `.git`, não a árvore de trabalho, para não gastar milhares de
  vigias com `node_modules`.
- **A cada 2 minutos, e quando a janela do hub abre:** todos (é assim que um arquivo
  editado aparece).
- **A cada 20 minutos, e no Atualizar:** procura repositórios novos nas pastas.
- **A cada 30 minutos:** o CI e os PRs pelo `gh`; e, com a opção "Buscar no remoto" ligada,
  `git fetch` em quem tem remoto, sem pedir senha (sem chave carregada, o fetch falha em
  silêncio e vai para o registro do hub).

O app usa o próprio `git` (precisa estar no PATH), com `--no-optional-locks`: a leitura
não mexe no repositório.
