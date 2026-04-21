import { Route, Routes } from 'react-router-dom'
import AppLayout from './components/app-layout'
import HomePage from './pages/home-page'
import LoginPage from './pages/login-page'
import TerminalPage from './pages/terminal-page'
import GitPage from './pages/git-page'
import FilesPage from './pages/files-page'
import PreviewPage from './pages/preview-page'

function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route element={<AppLayout />}>
        <Route path="/" element={<HomePage />} />
        <Route path="/terminal" element={<TerminalPage />} />
        <Route path="/git" element={<GitPage />} />
        <Route path="/files" element={<FilesPage />} />
        <Route path="/preview" element={<PreviewPage />} />
      </Route>
    </Routes>
  )
}

export default App
